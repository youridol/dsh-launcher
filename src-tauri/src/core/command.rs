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

/// 创建一个"无窗口"的 .cmd 命令执行器（适用于 npm / pnpm 等 batch 脚本）
/// 通过 `cmd.exe /C` 包装，cmd 会按 PATHEXT 正确解析 .cmd 并保持输出管道。
#[cfg(windows)]
pub fn hidden_cmd<S: AsRef<std::ffi::OsStr>>(program: S) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("cmd.exe");
    cmd.creation_flags(CREATE_NO_WINDOW);
    // /C 执行后退出；/D 忽略 AutoRun（避免用户注册表 AutoRun 干扰）
    cmd.arg("/D").arg("/C").arg(program.as_ref());
    cmd
}

/// 非 Windows 平台：.cmd 不存在，直接用 program（如 npm 的 shell 脚本）
#[cfg(not(windows))]
pub fn hidden_cmd<S: AsRef<std::ffi::OsStr>>(program: S) -> std::process::Command {
    std::process::Command::new(program)
}
