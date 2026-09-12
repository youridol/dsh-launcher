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

/// `run_streamed` 的失败详情：除「退出码」外，还带回**错误输出尾部**。
///
/// 审计 BUG-1：此前 `run_streamed` 失败时只返回 `命令退出码: 1`，子进程的
/// stderr（真正的失败原因，如 `ERR_PNPM_FETCH_404 ... is not in the npm registry`）
/// 只写进了日志，没有随错误上抛 → 前端只能显示「退出码 1」，用户无从自助定位。
#[derive(Debug, Clone, Default)]
pub struct StreamFailure {
    /// 进程无法启动/等待失败等 IO 错误（有值时直接以它为准）
    pub io_error: Option<String>,
    /// 子进程退出码（超时/无法取得时为 None）
    pub exit_code: Option<i32>,
    /// 是否因超时被强杀
    pub timed_out: bool,
    /// **stdout** 尾部。
    ///
    /// 为何连 stdout 也保留：本仓实测（pnpm 11.24）**把致命错误也写到 stdout**——
    /// 例如 `[ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED] … allowBuilds …` 完全不在 stderr。
    /// 只收 stderr 会导致失败原因再次丢失（正是 BUG-1 要修的问题）。
    pub stdout_tail: Vec<String>,
    /// **stderr** 尾部
    pub stderr_tail: Vec<String>,
}

impl StreamFailure {
    /// IO 类失败（启动失败 / 等待失败）
    pub fn io(message: impl Into<String>) -> Self {
        Self {
            io_error: Some(message.into()),
            ..Default::default()
        }
    }

    /// 合并 stdout + stderr 的诊断行（stdout 在前，因 pnpm 把错误写在 stdout）。
    ///
    /// 过滤 dsh 自身的尾部噪声行与纯空行；保留顺序以便阅读。
    fn diagnostic_lines(&self) -> Vec<&str> {
        self.stdout_tail
            .iter()
            .chain(self.stderr_tail.iter())
            .map(|line| line.trim_end())
            .filter(|line| !line.trim().is_empty() && !is_dsh_trailer(line))
            .collect()
    }

    /// 面向用户的单行说明：IO 错误 > 超时 > `命令退出码: N`（附最后一句诊断）。
    ///
    /// 仅用于兼容旧调用方（`run_streamed`）；需要完整尾部请看 [`Self::detail`]。
    pub fn message(&self) -> String {
        if let Some(error) = &self.io_error {
            return error.clone();
        }
        if self.timed_out {
            return "命令执行超时已被强制终止（见日志）".to_string();
        }
        let code = self.exit_code.unwrap_or(-1);
        match self.diagnostic_lines().last() {
            Some(last) => format!("命令退出码: {code}；{last}"),
            None => format!("命令退出码: {code}"),
        }
    }

    /// 多行详情（退出码 + 诊断行），供安装类操作直接展示给用户。
    pub fn detail(&self) -> String {
        if let Some(error) = &self.io_error {
            return error.clone();
        }
        let mut out = if self.timed_out {
            "命令超时被强制终止".to_string()
        } else {
            format!("命令退出码 {}", self.exit_code.unwrap_or(-1))
        };
        // 完整诊断文本（供关键词识别）
        let lines = self.diagnostic_lines();
        for line in &lines {
            out.push_str("\n  ");
            out.push_str(line);
        }
        out
    }

    /// 供关键词匹配的完整诊断文本（stdout + stderr，已过滤噪声）。
    pub fn diagnostics(&self) -> String {
        self.diagnostic_lines().join("\n")
    }
}

/// dsh 自身的尾部噪声行（非 pnpm 原始诊断），展示时过滤掉。
///
/// 例：`dsh: pnpm failed in profile directory …`、`dsh: initialized profile …`。
fn is_dsh_trailer(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("dsh: pnpm failed") || trimmed.starts_with("dsh: initialized profile")
}

/// 保留的 stdout/stderr 尾部行数上限
///
/// 取 40：pnpm 的 allowBuilds 报错块本身就有 5-7 行，且其后还会跟 dsh 的尾部说明；
/// 12 行会让**真正的原因被后期噪声挤掉**（本仓实测教训）。
const STDERR_TAIL_MAX_LINES: usize = 40;

/// 读取全部行并回调，同时把行尾记录入 `tail`（供失败诊断）。
fn read_all_lines_into_tail<R: std::io::Read>(
    reader: R,
    on_line: &dyn Fn(&str),
    tail: &Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
) {
    read_all_lines(reader, &|line| {
        on_line(line);
        if let Ok(mut guard) = tail.lock() {
            guard.push_back(line.to_string());
            while guard.len() > STDERR_TAIL_MAX_LINES {
                guard.pop_front();
            }
        }
    });
}

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
    cmd: Command,
    out_level: LogLevel,
    err_level: LogLevel,
    on_line: Option<Arc<LineCallback>>,
) -> Result<(), String> {
    run_streamed_checked(logger, cmd, out_level, err_level, on_line).map_err(|e| e.message())
}

/// 与 [`run_streamed`] 相同，但失败时返回**结构化的 [`StreamFailure`]**。
///
/// 审计 BUG-1：安装类操作需要把子进程 stderr（真正的失败原因）上抛给用户，
/// 而不是只报「命令退出码: 1」。
pub fn run_streamed_checked(
    logger: &Arc<Logger>,
    mut cmd: Command,
    out_level: LogLevel,
    err_level: LogLevel,
    on_line: Option<Arc<LineCallback>>,
) -> Result<(), StreamFailure> {
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let mut child = cmd
        .spawn()
        .map_err(|e| StreamFailure::io(format!("命令启动失败: {e}")))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let logger_out = Arc::clone(logger);
    let logger_err = Arc::clone(logger);
    let cb_out = on_line.clone();
    let cb_err = on_line.clone();
    // stderr 尾部缓冲（供失败诊断）
    let err_tail: Arc<std::sync::Mutex<std::collections::VecDeque<String>>> =
        Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    // stdout 尾部缓冲：本仓实测 pnpm 把**致命错误也写到 stdout**（见 StreamFailure 注释），
    // 故两侧都必须保留，否则失败原因仍会丢失。
    let out_tail: Arc<std::sync::Mutex<std::collections::VecDeque<String>>> =
        Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));

    // stdout 读取线程（同时记录尾部）
    let t_out = stdout.map(|pipe| {
        let tail = Arc::clone(&out_tail);
        let cb = cb_out.clone();
        thread::spawn(move || {
            let reader = BufReader::new(pipe);
            read_all_lines_into_tail(
                reader,
                &|line| {
                    logger_out.log(LogSource::Launcher, out_level, line);
                    if let Some(cb) = &cb {
                        cb(out_level, line);
                    }
                },
                &tail,
            );
        })
    });

    // stderr 读取线程（同时记录尾部）
    let t_err = stderr.map(|pipe| {
        let tail = Arc::clone(&err_tail);
        let cb = cb_err.clone();
        thread::spawn(move || {
            let reader = BufReader::new(pipe);
            read_all_lines_into_tail(
                reader,
                &|line| {
                    logger_err.log(LogSource::Launcher, err_level, line);
                    if let Some(cb) = &cb {
                        cb(err_level, line);
                    }
                },
                &tail,
            );
        })
    });

    // 等待子进程结束（不依赖读取线程，避免管道阻塞时 wait 挂起）；
    // v0.4.13：看门狗轮询 + 总时长上限，超时强杀进程树（防永久挂起）。
    let started = Instant::now();
    let mut timed_out = false;
    let mut wait_error: Option<String> = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {}
            Err(e) => {
                wait_error = Some(format!("等待命令退出失败: {e}"));
                break None;
            }
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
            // 强杀后等待进程真正退出（回收句柄）。
            // v0.4.15（审计修复）：加 3s 上限，防止 taskkill 失败且进程不退时
            // try_wait 永不 Some → 调用线程死循环卡死。
            let kill_deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {}
                    Err(e) => {
                        wait_error = Some(format!("等待命令退出失败: {e}"));
                        break;
                    }
                }
                if Instant::now() >= kill_deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            if wait_error.is_some() {
                break None;
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

    // 汇总 stdout/stderr 尾部（读线程已结束，锁不可能长期被占）
    let stdout_tail: Vec<String> = out_tail
        .lock()
        .map(|guard| guard.iter().cloned().collect())
        .unwrap_or_default();
    let stderr_tail: Vec<String> = err_tail
        .lock()
        .map(|guard| guard.iter().cloned().collect())
        .unwrap_or_default();

    if let Some(error) = wait_error {
        return Err(StreamFailure {
            io_error: Some(error),
            stdout_tail,
            stderr_tail,
            ..Default::default()
        });
    }
    if timed_out {
        return Err(StreamFailure {
            timed_out: true,
            stdout_tail,
            stderr_tail,
            ..Default::default()
        });
    }
    match status {
        Some(st) if st.success() => Ok(()),
        Some(st) => Err(StreamFailure {
            exit_code: Some(st.code().unwrap_or(-1)),
            stdout_tail,
            stderr_tail,
            ..Default::default()
        }),
        None => Err(StreamFailure {
            io_error: Some("未能取得命令退出状态".to_string()),
            stdout_tail,
            stderr_tail,
            ..Default::default()
        }),
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
