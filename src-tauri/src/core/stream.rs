//! 流式命令执行器：逐行读取子进程 stdout/stderr → 写入日志并推送前端
//!
//! 解决的问题（v0.1.7）：
//! - 安装（npm/git/pnpm 等）此前用 `Command::output()` 同步等待，过程完全静默：
//!   既不写日志（无落盘、无实时流），也无进度反馈 → 用户感知"无响应"。
//! - process.rs 此前"先读完全部 stdout 再读 stderr"：若 stdout 管道长期无数据且进程
//!   存活，stderr 数据永远不被读取 → 日志流缺失。本模块用两个独立线程并行读取。
//!
//! 用法（示例，非可执行代码）：
//! 1. 构造 Command（可经 command::hidden / command::hidden_cmd）
//! 2. 调用 run_streamed(&logger, cmd, out_level, err_level, on_line)
//! 3. on_line 回调可解析输出行（如 git clone 百分比）并推进度事件

use crate::core::command;
use crate::core::logging::{LogLevel, LogSource, Logger};
use std::io::BufReader;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// 流式命令总执行时长上限（30 分钟）。
/// v0.4.13（审计修复 2.7/2.8）：npm/pnpm/git 等安装/构建此前无任何超时，
/// 网络卡死时前端永久挂起；超时由看门狗强杀进程树。
const STREAMED_MAX_DURATION: Duration = Duration::from_secs(30 * 60);
/// 进程退出后等待读线程收尾的宽限（孙进程偶发持有管道时避免永久 join）
const DRAIN_GRACE: Duration = Duration::from_secs(8);

/// 行回调：安装流程可在此解析输出行（如 git clone 百分比）并推进度事件。
/// 返回 false 可提前终止读取（一般不用）。
pub type LineCallback = dyn Fn(LogLevel, &str) + Send + Sync + 'static;

/// 以流式方式运行命令并等待完成。
///
/// - stdout 行按 `out_level` 写日志；stderr 行按 `err_level` 写日志
///   （`err_level` 默认 Warn，调用方可传 LogLevel::Info 避免把正常进度当警告）
/// - 两个独立线程并行读取 stdout/stderr，避免管道互堵
/// - 每行都会 `Logger::log` → 落盘 + emit `log://line` 前端实时流
/// - 可选 `on_line` 回调（在写日志后调用），用于安装流程推进度
/// - 分隔符：`\n` 与 `\r` 均视为行分隔（git 进度用 `\r` 刷新同一行）
pub fn run_streamed(
    logger: &Arc<Logger>,
    mut cmd: Command,
    out_level: LogLevel,
    err_level: LogLevel,
    on_line: Option<Arc<LineCallback>>,
) -> Result<(), String> {
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("命令启动失败: {e}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let logger_out = Arc::clone(logger);
    let logger_err = Arc::clone(logger);
    let cb_out = on_line.clone();
    let cb_err = on_line.clone();

    // stdout 读取线程
    let t_out = stdout.map(|pipe| {
        thread::spawn(move || {
            let reader = BufReader::new(pipe);
            read_all_lines(reader, &|line| {
                logger_out.log(LogSource::Launcher, out_level, line);
                if let Some(cb) = &cb_out {
                    cb(out_level, line);
                }
            });
        })
    });

    // stderr 读取线程
    let t_err = stderr.map(|pipe| {
        thread::spawn(move || {
            let reader = BufReader::new(pipe);
            read_all_lines(reader, &|line| {
                logger_err.log(LogSource::Launcher, err_level, line);
                if let Some(cb) = &cb_err {
                    cb(err_level, line);
                }
            });
        })
    });

    // 等待子进程结束（不依赖读取线程，避免管道阻塞时 wait 挂起）；
    // v0.4.13：看门狗轮询 + 总时长上限，超时强杀进程树（防永久挂起）。
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {}
            Err(e) => return Err(format!("等待命令退出失败: {e}")),
        }
        if Instant::now() - started >= STREAMED_MAX_DURATION {
            timed_out = true;
            logger.log(
                LogSource::Launcher,
                LogLevel::Error,
                &format!(
                    "命令执行超过 {} 分钟，强制终止（pid={}）",
                    STREAMED_MAX_DURATION.as_secs() / 60,
                    child.id()
                ),
            );
            command::kill_process_tree(child.id());
            // 强杀后等待进程真正退出（回收句柄）
            loop {
                if child
                    .try_wait()
                    .map_err(|e| format!("等待命令退出失败: {e}"))?
                    .is_some()
                {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            continue;
        }
        thread::sleep(Duration::from_millis(200));
    };

    // 等待两个读取线程收尾（带宽限，避免孙进程持有管道时永久 join）
    if let Some(t) = t_out {
        join_with_grace(t, "stdout 读取线程");
    }
    if let Some(t) = t_err {
        join_with_grace(t, "stderr 读取线程");
    }

    if timed_out {
        return Err("命令执行超时已被强制终止（见日志）".to_string());
    }
    if status.success() {
        Ok(())
    } else {
        Err(format!("命令退出码: {}", status.code().unwrap_or(-1)))
    }
}

/// 带宽限的线程 join：正常情况 8s 内未结束则放弃等待（drop 句柄）。
/// 触发条件极罕见（子进程的孙进程继承了输出管道句柄），放弃等待可避免
/// 安装/构建流程被一个残留后台进程永久卡住；进程树被杀后管道最终会关闭。
fn join_with_grace(handle: thread::JoinHandle<()>, what: &str) {
    let deadline = Instant::now() + DRAIN_GRACE;
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    if handle.is_finished() {
        let _ = handle.join();
    } else {
        eprintln!("[dsh-launcher] {what} 未在宽限内结束，放弃等待（避免永久挂起）");
    }
}

/// 读取全部行并回调：`\n` 与 `\r` 均作为行分隔符（git 进度用 `\r` 刷新同一行）。
///
/// - 逐字节缓冲，遇到 `\r` 或 `\n` 立即切行回调（实时）：
///   git 的 `\r` 进度刷新、npm/pnpm 的 `\n` 普通行都能实时拿到。
/// - 缓冲跨行保留（不丢数据）；EOF 时的尾行（无换行）也回调。
/// - 空行不回调（避免 \r\n 连排产生空行）。
/// - **编码修复**：行按 UTF-8 严格解码，失败回退 Windows 代码页（GBK/OEM）——
///   cmd/npm 批处理在中文系统输出的 GBK 中文不再乱码（见 core/text.rs）。
pub(crate) fn read_all_lines<R: std::io::Read>(mut reader: R, on_line: &dyn Fn(&str)) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = reader.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        for &b in &chunk[..n] {
            if b == b'\r' || b == b'\n' {
                if !buf.is_empty() {
                    let line = crate::core::text::decode(&buf).trim().to_string();
                    if !line.is_empty() {
                        on_line(&line);
                    }
                    buf.clear();
                }
            } else {
                buf.push(b);
            }
        }
    }
    // EOF 尾行
    if !buf.is_empty() {
        let line = crate::core::text::decode(&buf).trim().to_string();
        if !line.is_empty() {
            on_line(&line);
        }
    }
}

/// 便捷封装：运行一个 .cmd 脚本（npm/pnpm）并流式输出
pub fn run_cmd_script(
    logger: &Arc<Logger>,
    program: &str,
    args: &[String],
    cwd: Option<&std::path::Path>,
    err_level: LogLevel,
    on_line: Option<Arc<LineCallback>>,
) -> Result<(), String> {
    let mut c = command::hidden_cmd(program);
    c.args(args);
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    run_streamed(logger, c, LogLevel::Info, err_level, on_line)
}

#[cfg(test)]
mod tests {
    use super::read_all_lines;
    use std::io::Cursor;

    fn collect(data: &[u8]) -> Vec<String> {
        let lines = std::cell::RefCell::new(Vec::new());
        let reader = Cursor::new(data);
        read_all_lines(reader, &|line| {
            lines.borrow_mut().push(line.to_string());
        });
        lines.into_inner()
    }

    #[test]
    fn test_read_all_lines_git_progress() {
        // git 进度：\r 结尾实时刷新（无 \n），最后 \n 收尾
        let data = b"Receiving objects:  0% (1/10234)\rReceiving objects:  45% (45/10234)\rReceiving objects: 100% (10234/10234), done.\n";
        let lines = collect(data);
        assert_eq!(
            lines,
            vec![
                "Receiving objects:  0% (1/10234)",
                "Receiving objects:  45% (45/10234)",
                "Receiving objects: 100% (10234/10234), done.",
            ]
        );
    }

    #[test]
    fn test_read_all_lines_normal_newline() {
        // 普通行：\n 结尾，末尾无换行
        let data = b"line1\nline2\nline3 no trailing newline";
        let lines = collect(data);
        assert_eq!(lines, vec!["line1", "line2", "line3 no trailing newline"]);
    }

    #[test]
    fn test_read_all_lines_crlf_mixed() {
        // 混合：\r\n 与 \r，空行被丢弃
        let data = b"first\r\nsecond\rthird\r\nfourth\n";
        let lines = collect(data);
        assert_eq!(lines, vec!["first", "second", "third", "fourth"]);
    }

    #[test]
    fn test_real_git_output_segment() {
        // 真实 git clone 输出片段（从实测抓取）：\r 刷新 + \n 收尾
        let data = b"Cloning into 'clone-test'...\nremote: Enumerating objects: 10234, done.\nReceiving objects:   0% (1/10234)\rReceiving objects:   1% (103/10234), 56.00 KiB | 79.00 KiB/s\rReceiving objects:  45% (4606/10234), 5.82 MiB | 479.00 KiB/s\rReceiving objects: 100% (10234/10234), 18.02 MiB | 522.00 KiB/s, done.\nResolving deltas: 100% (2685/2685), done.\n";
        let lines = collect(data);
        // 每一条 \r 进度刷新区分出来（6 个分隔符 → 7 行）
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[0], "Cloning into 'clone-test'...");
        assert_eq!(lines[1], "remote: Enumerating objects: 10234, done.");
        assert!(lines[2].contains("Receiving objects:   0%"));
        assert!(lines[3].contains("Receiving objects:   1%"));
        assert!(lines[4].contains("Receiving objects:  45%"));
        assert!(lines[5].contains("Receiving objects: 100%"));
        assert_eq!(lines[6], "Resolving deltas: 100% (2685/2685), done.");
    }
}
