//! 统一进程创建辅助：禁止弹出黑色控制台窗口
//!
//! Windows 下所有 spawn 的子进程（dsh、npm、git、pnpm、curl、taskkill、powershell）
//! 默认会继承控制台或弹出新窗口。这里统一应用 `CREATE_NO_WINDOW` (0x08000000)：
//! 不创建新控制台窗口，但保留标准输出/错误管道。
//!
//! ⚠️ 注意：不能使用 `DETACHED_PROCESS` (0x00000008) —— 实测该标志会导致
//! npm.cmd 等 batch 脚本的子进程（node.exe）输出管道失效（stdout 为空），
//! 导致 `npm view` / `npm install` 等命令"成功退出但无输出"。见 tests/npm_versions_test.rs。
//!
//! npm/pnpm 在 Windows 上是 .cmd（batch），`Command::new("npm")` 找不到可执行文件，
//! 必须用 `cmd.exe /C npm ...` 包装（cmd 会按 PATHEXT 解析 .cmd）。
//! 本模块提供 `hidden_cmd()` 辅助完成包装。
//!
//! 用法：
//! - 直接执行 .exe：`let mut cmd = command::hidden("git");`
//! - 执行 .cmd（npm/pnpm）：`let mut cmd = command::hidden_cmd("npm"); cmd.args([...]);`

/// CREATE_NO_WINDOW：不创建控制台窗口
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 创建一个已配置为"无窗口"的 Command（适用于 .exe 程序）
#[cfg(windows)]
pub fn hidden<S: AsRef<std::ffi::OsStr>>(program: S) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// 非 Windows 平台：普通 Command
#[cfg(not(windows))]
pub fn hidden<S: AsRef<std::ffi::OsStr>>(program: S) -> std::process::Command {
    std::process::Command::new(program)
}

/// 创建一个"无窗口"的 .cmd 命令执行器（适用于 npm / pnpm / dsh 等 batch 脚本）
/// 通过 `cmd.exe /C` 包装，cmd 会按 PATHEXT 正确解析 .cmd 并保持输出管道。
///
/// v0.4.0：统一注入用户级 Node 目录到 PATH —— npm/pnpm/dsh 运行时需要 node，
/// 分发机器（本启动器安装 Node，仅写入用户 PATH）当前进程环境快照不含新 PATH，
/// 必须进程内前缀注入，否则 cmd /C npm 报 "'npm' 不是内部或外部命令"（退出码 1）。
#[cfg(windows)]
pub fn hidden_cmd<S: AsRef<std::ffi::OsStr>>(program: S) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("cmd.exe");
    cmd.creation_flags(CREATE_NO_WINDOW);
    // /C 执行后退出；/D 忽略 AutoRun（避免用户注册表 AutoRun 干扰）
    cmd.arg("/D").arg("/C").arg(program.as_ref());
    // 注入用户级 node_dir（幂等：PATH 已有则跳过；探测一次 where node，开销极小）
    crate::core::pathutil::inject_node_path_into(&mut cmd);
    cmd
}

/// 非 Windows 平台：.cmd 不存在，直接用 program（如 npm 的 shell 脚本）
#[cfg(not(windows))]
pub fn hidden_cmd<S: AsRef<std::ffi::OsStr>>(program: S) -> std::process::Command {
    std::process::Command::new(program)
}

// ==================== 超时与取消（审计修复 2.8）====================
// 此前全部子进程调用使用阻塞 .output()/wait() 且无超时：网络黑洞/远端卡死时
// 安装与查询永久挂起、前端无限等待。以下提供统一的“带超时执行 + 超时强杀进程树”。

/// 强杀进程树（Windows: taskkill /T /F；其他平台: 直接 kill）
/// pub(crate)：供 stream.rs 看门狗与 commands 层超时清剿复用。
pub(crate) fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("taskkill");
        c.args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000);
        let _ = c.output();
    }
    #[cfg(not(windows))]
    {
        // 无 /T 语义：仅杀直接子进程（尽力而为）
        if let Ok(mut child) = std::process::Command::new("kill")
            .arg("-9")
            .arg(pid.to_string())
            .spawn()
        {
            let _ = child.wait();
        }
    }
}

/// 带超时执行命令并完整捕获 stdout/stderr（管道并发读取，避免互堵）。
/// 超时后强杀进程树并返回 io::ErrorKind::TimedOut。
///
/// 用法：`let out = run_with_timeout(cmd, Duration::from_secs(120))?;`
pub fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::Instant;

    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let mut child = cmd.spawn()?;
    let pid = child.id();

    // 两个读线程并行收集输出（与 stream.rs 同理，防管道互堵）
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let t_out = stdout.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });
    let t_err = stderr.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });

    // 轮询等待退出，超时则强杀
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if Instant::now() >= deadline {
            kill_process_tree(pid);
            // 强杀后等待进程真正退出（回收句柄）。
            // v0.4.15（审计修复）：加 3s 上限，防止 taskkill 失败（权限/被占）且
            // 进程不退时 try_wait 永不 Some → 调用线程死循环卡死。
            let kill_deadline = Instant::now() + std::time::Duration::from_secs(3);
            loop {
                if child.try_wait()?.is_some() {
                    break;
                }
                if Instant::now() >= kill_deadline {
                    break; // 尽力而为：句柄由进程退出时系统回收
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("命令执行超过 {}s 被强制终止（pid={pid}）", timeout.as_secs()),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };

    let stdout = t_out.map(|t| t.join().unwrap_or_default()).unwrap_or_default();
    let stderr = t_err.map(|t| t.join().unwrap_or_default()).unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}
