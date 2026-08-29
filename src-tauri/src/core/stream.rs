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
use std::io::{BufRead, BufReader};
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
            for line in reader.lines().map_while(Result::ok) {
                let line = line.trim_end_matches('\r').to_string();
                logger_out.log(LogSource::Launcher, out_level, &line);
                if let Some(cb) = &cb_out {
                    cb(out_level, &line);
                }
            }
        })
    });

    // stderr 读取线程
    let t_err = stderr.map(|pipe| {
        thread::spawn(move || {
            let reader = BufReader::new(pipe);
            for line in reader.lines().map_while(Result::ok) {
                let line = line.trim_end_matches('\r').to_string();
                logger_err.log(LogSource::Launcher, err_level, &line);
                if let Some(cb) = &cb_err {
                    cb(err_level, &line);
                }
            }
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

/// 便捷封装：运行一个"无窗口"的 .exe 命令并流式输出（如 git）
pub fn run_exe(
    logger: &Arc<Logger>,
    program: &str,
    args: &[String],
    cwd: Option<&std::path::Path>,
    err_level: LogLevel,
    on_line: Option<Arc<LineCallback>>,
) -> Result<(), String> {
    let mut c = command::hidden(program);
    c.args(args);
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    run_streamed(logger, c, LogLevel::Info, err_level, on_line)
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
