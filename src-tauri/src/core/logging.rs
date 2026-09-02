//! 全域日志：dsh 进程 stdout/stderr + 启动器自身日志统一落盘
//!
//! 策略（docs/DESIGN.md §4.4）：
//! - 目录 `%LOCALAPPDATA%\dsh-launcher\logs\`，当前布局为单文件 `<date>.log` 直接放根目录
//! - 单文件按大小 10MB 切割（.log.1、.log.2 …，级联轮转，见 rotate()）
//! - 按天轮转，保留最近 30 天（cleanup_old 兼容早期 `<date>/<date>.log` 目录布局）
//! - 前端通过 Tauri event 流式接收（见 commands/logs.rs）

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Emitter;

use crate::core::events::{self, InstallPhase};

/// 前端日志流事件名
pub const LOG_EVENT: &str = "log://line";

/// 单文件大小上限：10MB
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;
/// 日志保留天数
const RETAIN_DAYS: i64 = 30;
/// 单文件切割后保留的旧文件份数
const MAX_ROTATED: u32 = 5;

/// 日志来源
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSource {
    /// 启动器自身
    Launcher,
    /// dsh 进程 stdout/stderr
    Dsh,
}

impl LogSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogSource::Launcher => "launcher",
            LogSource::Dsh => "dsh",
        }
    }
}

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

/// 全局日志写入器（线程安全）
pub struct Logger {
    inner: Mutex<LoggerInner>,
    /// Tauri AppHandle（用于向前端推送日志流；None 表示尚未关联）
    emitter: Mutex<Option<tauri::AppHandle>>,
}

/// 日志文件状态
struct LoggerInner {
    dir: PathBuf,
    current_date: String,
    current_size: u64,
}

/// 推送给前端的一条日志行
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub timestamp: String,
    pub source: &'static str,
    pub level: &'static str,
    pub message: String,
}

impl Logger {
    /// 初始化日志系统，返回全局单例
    pub fn init() -> std::io::Result<Self> {
        let dir = logs_dir();
        fs::create_dir_all(&dir)?;
        let logger = Logger {
            inner: Mutex::new(LoggerInner {
                dir,
                current_date: String::new(),
                current_size: 0,
            }),
            emitter: Mutex::new(None),
        };
        logger.cleanup_old();
        Ok(logger)
    }

    /// 关联 Tauri AppHandle（启动时调用一次）
    pub fn set_emitter(&self, app: tauri::AppHandle) {
        if let Ok(mut e) = self.emitter.lock() {
            *e = Some(app);
        }
    }

    /// 便捷方法：启动器自身 INFO 日志（source 固定 Launcher）
    pub fn info(&self, msg: &str) {
        self.log(LogSource::Launcher, LogLevel::Info, msg);
    }

    /// 便捷方法：启动器自身 WARN 日志
    pub fn warn(&self, msg: &str) {
        self.log(LogSource::Launcher, LogLevel::Warn, msg);
    }

    /// 便捷方法：启动器自身 ERROR 日志
    pub fn error(&self, msg: &str) {
        self.log(LogSource::Launcher, LogLevel::Error, msg);
    }

    /// 写入一条日志
    pub fn log(&self, source: LogSource, level: LogLevel, msg: &str) {
        let timestamp = {
            let Ok(mut inner) = self.inner.lock() else {
                return;
            };
            let now = chrono_now();
            let date = now.date;
            // 日期变化 → 切换新文件
            if inner.current_date != date {
                inner.current_date = date.clone();
                inner.current_size = 0;
            }
            let path = inner.dir.join(format!("{date}.log"));
            let line = format!(
                "[{}] [{}] [{}] {}\n",
                now.timestamp,
                source.as_str(),
                level.as_str(),
                msg
            );
            // 超限切割（级联轮转 .log.1 … .log.MAX，见 rotate()）
            if inner.current_size + line.len() as u64 > MAX_FILE_SIZE {
                rotate(&path);
                inner.current_size = 0;
            }
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
                if f.write_all(line.as_bytes()).is_ok() {
                    inner.current_size += line.len() as u64;
                }
            }
            now.timestamp
        };
        // 锁外推送前端（避免死锁）
        self.emit_line(timestamp, source, level, msg);
    }

    /// 推送到前端一条日志行（内部使用，写入文件后调用）
    fn emit_line(&self, timestamp: String, source: LogSource, level: LogLevel, msg: &str) {
        if let Ok(emitter) = self.emitter.lock() {
            if let Some(app) = emitter.as_ref() {
                let _ = app.emit(
                    LOG_EVENT,
                    LogLine {
                        timestamp,
                        source: source.as_str(),
                        level: level.as_str(),
                        message: msg.to_string(),
                    },
                );
            }
        }
    }

    /// 推送到前端一条安装进度事件（安装流程在后台线程调用）
    pub fn progress(
        &self,
        channel: &str,
        phase: InstallPhase,
        percent: u8,
        message: impl Into<String>,
    ) {
        let payload = events::progress(channel, phase, percent, message);
        if let Ok(emitter) = self.emitter.lock() {
            if let Some(app) = emitter.as_ref() {
                events::emit(app, &payload);
            }
        }
    }

    /// 删除超过 RETAIN_DAYS 天前的日志文件（当前布局：logs/<date>.log 直接放根目录）
    fn cleanup_old(&self) {
        let Ok(inner) = self.inner.lock() else {
            return;
        };
        let today = chrono_now().date;
        let Ok(entries) = fs::read_dir(&inner.dir) else {
            return;
        };
        for entry in entries.flatten() {
            // 兼容早期日期目录（logs/<date>/<date>.log）与当前单文件布局（logs/<date>.log）
            if entry.path().is_dir() {
                // 早期布局：目录名形如 yyyy-mm-dd，删除整目录
                let name = entry.file_name().to_string_lossy().to_string();
                if name.len() == 10 && is_older_than(&name, &today, RETAIN_DAYS) {
                    let _ = fs::remove_dir_all(entry.path());
                }
                continue;
            }
            // 当前布局：文件名形如 yyyy-mm-dd.log / yyyy-mm-dd.log.N（切割旧档）
            let name = entry.file_name().to_string_lossy().to_string();
            let base = name
                .strip_suffix(".log")
                .or_else(|| name.split_once(".log.").map(|(b, _)| b))
                .unwrap_or("");
            if base.len() == 10 && is_older_than(base, &today, RETAIN_DAYS) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// 判断日期字符串（yyyy-mm-dd）是否比 today 早超过 days 天
fn is_older_than(date: &str, today: &str, days: i64) -> bool {
    let Some(d) = date_to_days(date) else {
        return false;
    };
    let Some(t) = date_to_days(today) else {
        return false;
    };
    t.saturating_sub(d) > days
}

/// 将 yyyy-mm-dd 转为自 Unix 纪元的整数天数
fn date_to_days(date: &str) -> Option<i64> {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let y: i64 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let d: u32 = parts[2].parse().ok()?;
    // 公历日期转天数（Gompertz 式算法）
    let a = (14 - m as i64) / 12;
    let y2 = y + 4800 - a;
    let m2 = m as i64 + 12 * a - 3;
    // 公历转儒略日数（JDN），再减去 Unix 纪元偏移 2440588 得到自 1970-01-01 的天数
    let jdn = d as i64 + (153 * m2 + 2) / 5 + 365 * y2 + y2 / 4 - y2 / 100 + y2 / 400 - 32045;
    Some(jdn - 2440588)
}

/// 当前时间快照
struct Now {
    /// 日期 yyyy-mm-dd
    date: String,
    /// 时间戳 HH:MM:SS
    timestamp: String,
}

/// 当前时间（简单实现，避免额外依赖 chrono 时的编译负担）
fn chrono_now() -> Now {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    let date = epoch_to_date(days as i64);
    let timestamp = format!(
        "{:02}:{:02}:{:02}",
        (secs % 86400) / 3600,
        (secs % 3600) / 60,
        secs % 60
    );
    Now { date, timestamp }
}

/// 将 Unix 天数转为 yyyy-mm-dd（公历，简单算法）
fn epoch_to_date(days: i64) -> String {
    // 1970-01-01 = day 0（周四）
    let mut y = 1970i64;
    let mut d = days;
    loop {
        let leap = is_leap(y);
        let dim = if leap { 366 } else { 365 };
        if d < dim {
            break;
        }
        d -= dim;
        y += 1;
    }
    let leap = is_leap(y);
    let mdays = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0usize;
    for (i, &md) in mdays.iter().enumerate() {
        if d < md {
            m = i;
            break;
        }
        d -= md;
    }
    format!("{:04}-{:02}-{:02}", y, m + 1, d + 1)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// 日志根目录：%LOCALAPPDATA%\dsh-launcher\logs
/// pub：commands/logs.rs 列表/读取复用（唯一实现，避免两处重复）
pub fn logs_dir() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("dsh-launcher")
        .join("logs")
}

/// 从最新日志文件中提取最近一次 dsh web 启动的完整 URL（含 token）
/// 兜底方案：内存捕获失败时（如重启后）从落盘日志恢复。
pub fn extract_latest_web_url() -> Option<String> {
    let dir = logs_dir();
    let today = chrono_now().date;
    let path = dir.join(format!("{today}.log"));
    let Ok(content) = fs::read_to_string(&path) else {
        return None;
    };
    find_web_url_in_content(&content)
}

/// 从文本中提取最后出现的带 token 的 dsh web URL
fn find_web_url_in_content(content: &str) -> Option<String> {
    // 从后往前找 "dsh web: http://127.0.0.1:<port>/?token=" 行（最后出现的 URL）
    content
        .lines()
        .rev()
        .find_map(|line| {
            let start = line.find("http://127.0.0.1:")?;
            let rest = &line[start..];
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '\r')
                .unwrap_or(rest.len());
            let url = &rest[..end];
            if url.contains("token=") {
                Some(url.to_string())
            } else {
                None
            }
        })
}

/// 文件切割：当前 <date>.log 超限时，将旧文件按 .log.1 → .log.2 → … 级联轮转
/// （MAX_ROTATED 份为上限：先删最旧 .log.MAX，再依次后移，最后把当前文件变为 .log.1）
/// 注意：这不是“循环复用”覆盖，而是归档式轮换——每份切割档保留唯一内容，
/// 超过上限的最旧档被删除，保证同一天最多保留 MAX_ROTATED 份历史切割档。
fn rotate(path: &Path) {
    // 先删最旧的 .log.MAX（若存在）
    let oldest = path.with_extension(format!("log.{}", MAX_ROTATED));
    let _ = fs::remove_file(&oldest);
    // 级联后移：.log.{MAX-1} → .log.{MAX} … .log.1 → .log.2
    for i in (1..MAX_ROTATED).rev() {
        let from = path.with_extension(format!("log.{i}"));
        let to = path.with_extension(format!("log.{}", i + 1));
        let _ = fs::rename(&from, &to);
    }
    // 当前文件 → .log.1
    let _ = fs::rename(path, path.with_extension("log.1"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_web_url_in_content() {
        // 真实日志格式：[HH:MM:SS] [dsh] [INFO] dsh web: http://127.0.0.1:3080/?token=abc
        let content = "[10:00:00] [launcher] [INFO] dsh 已启动\n[10:00:03] [dsh] [INFO] dsh web: http://127.0.0.1:3080/?token=abc123\n[10:00:03] [dsh] [INFO] dsh web: opening the default browser\n";
        assert_eq!(
            find_web_url_in_content(content).as_deref(),
            Some("http://127.0.0.1:3080/?token=abc123")
        );
        // 多条时取最后一条
        let multi = "[10:00:03] [dsh] [INFO] dsh web: http://127.0.0.1:3080/?token=one\n[11:00:03] [dsh] [INFO] dsh web: http://127.0.0.1:3080/?token=two\n";
        assert_eq!(find_web_url_in_content(multi).as_deref(), Some("http://127.0.0.1:3080/?token=two"));
        // 无 token 不提取
        assert_eq!(find_web_url_in_content("listening on http://127.0.0.1:3080"), None);
        assert_eq!(find_web_url_in_content(""), None);
    }

    #[test]
    fn test_date_to_days() {
        // 1970-01-01 = day 0
        assert_eq!(date_to_days("1970-01-01"), Some(0));
        // 1970-01-02 = day 1
        assert_eq!(date_to_days("1970-01-02"), Some(1));
        // 2024-01-01 应在合理范围（约 19723 天）
        let d = date_to_days("2024-01-01").unwrap();
        assert!(d > 19700 && d < 19740, "2024-01-01 = {d}");
    }

    #[test]
    fn test_is_older_than() {
        // 今天 2026-08-29，31 天前应判定为旧
        assert!(is_older_than("2026-07-28", "2026-08-29", 30));
        // 30 天内不算旧
        assert!(!is_older_than("2026-08-01", "2026-08-29", 30));
        // 非法日期不判定为旧
        assert!(!is_older_than("bad-date", "2026-08-29", 30));
    }

    #[test]
    fn test_rotate_cascades_and_keeps_max() {
        // 构造临时目录，验证切割后：.log.1 最新、旧档依次后移、超过 MAX_ROTATED 的最旧被删除
        let tmp = std::env::temp_dir().join(format!("dsh-log-rotate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("2026-09-02.log");
        fs::write(&path, "current").unwrap();
        // 预置旧档 .log.1 … .log.MAX（模拟历史切割）
        for i in 1..=MAX_ROTATED {
            fs::write(path.with_extension(format!("log.{i}")), format!("old{i}")).unwrap();
        }
        rotate(&path);
        // 当前文件已被移走 → 不存在
        assert!(!path.exists());
        // .log.1 = 原当前文件
        assert_eq!(fs::read_to_string(path.with_extension("log.1")).unwrap(), "current");
        // 其余级联后移：.log.{i} = 原 .log.{i-1}
        for i in 2..=MAX_ROTATED {
            assert_eq!(
                fs::read_to_string(path.with_extension(format!("log.{i}"))).unwrap(),
                format!("old{}", i - 1)
            );
        }
        // 最旧的 .log.MAX 原内容（old{MAX}）应已被删除（被 old{MAX-1} 顶替）
        assert_ne!(
            fs::read_to_string(path.with_extension(format!("log.{}", MAX_ROTATED))).unwrap(),
            format!("old{MAX_ROTATED}")
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_rotate_no_existing_backups() {
        // 无任何旧档时切割：仅生成 .log.1，不报错
        let tmp = std::env::temp_dir().join(format!("dsh-log-rotate2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("2026-09-02.log");
        fs::write(&path, "hello").unwrap();
        rotate(&path);
        assert_eq!(fs::read_to_string(path.with_extension("log.1")).unwrap(), "hello");
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_epoch_to_date_roundtrip() {
        let s = epoch_to_date(19723);
        assert!(s.starts_with("2024-"), "19723 天 = {s}");
    }
}
