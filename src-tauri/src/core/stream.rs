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

    // 等待子进程结束（不依赖读取线程，避免管道阻塞时 wait 挂起）
    let status = child.wait().map_err(|e| format!("等待命令退出失败: {e}"))?;

    // 等待两个读取线程收尾
    if let Some(t) = t_out {
        let _ = t.join();
    }
    if let Some(t) = t_err {
        let _ = t.join();
    }

    if status.success() {
        Ok(())
    } else {
        Err(format!("命令退出码: {}", status.code().unwrap_or(-1)))
    }
}

/// 读取全部行并回调：`\n` 与 `\r` 均作为行分隔符（git 进度用 `\r` 刷新同一行）。
///
/// - 逐字节缓冲，遇到 `\r` 或 `\n` 立即切行回调（实时）：
///   git 的 `\r` 进度刷新、npm/pnpm 的 `\n` 普通行都能实时拿到。
/// - 缓冲跨行保留（不丢数据）；EOF 时的尾行（无换行）也回调。
/// - 空行不回调（避免 \r\n 连排产生空行）。
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
                    let line = String::from_utf8_lossy(&buf).trim().to_string();
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
        let line = String::from_utf8_lossy(&buf).trim().to_string();
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
