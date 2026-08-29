//! 全域日志：dsh 进程 stdout/stderr + 启动器自身日志统一落盘
//!
//! 策略（docs/DESIGN.md §4.4）：
//! - 目录 `%LOCALAPPDATA%\dsh-launcher\logs\<yyyy-mm-dd>\`
//! - 单文件按大小 10MB 切割（.log.1、.log.2 …）
//! - 按天轮转，保留最近 30 天
//! - 前端通过 Tauri event 流式接收（见 commands/logs.rs）

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Emitter;

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
    current_index: u32,
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
                current_index: 0,
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
                inner.current_index = 0;
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
            // 超限切割
            if inner.current_size + line.len() as u64 > MAX_FILE_SIZE {
                rotate(&path, inner.current_index);
                inner.current_index += 1;
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
        if let Ok(emitter) = self.emitter.lock() {
            if let Some(app) = emitter.as_ref() {
                let _ = app.emit(
                    LOG_EVENT,
                    LogLine {
                        timestamp: timestamp.clone(),
                        source: source.as_str(),
                        level: level.as_str(),
                        message: msg.to_string(),
                    },
                );
            }
        }
    }

    /// 删除超过 RETAIN_DAYS 天前的日志目录
    fn cleanup_old(&self) {
        let Ok(inner) = self.inner.lock() else {
            return;
        };
        let today = chrono_now().date;
        let Ok(entries) = fs::read_dir(&inner.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // 目录名形如 yyyy-mm-dd，字典序即时间序
            if name.len() == 10 && is_older_than(&name, &today, RETAIN_DAYS) {
                let _ = fs::remove_dir_all(entry.path());
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
fn logs_dir() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("dsh-launcher")
        .join("logs")
}

/// 文件切割：把 path 重命名为 path.index 形式
fn rotate(path: &Path, index: u32) {
    if index >= MAX_ROTATED {
        // 超出保留份数，删除最旧的
        let old = path.with_extension(format!("log.{}", MAX_ROTATED));
        let _ = fs::remove_file(&old);
        return;
    }
    let new = path.with_extension(format!("log.{}", index));
    let _ = fs::rename(path, &new);
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_epoch_to_date_roundtrip() {
        let s = epoch_to_date(19723);
        assert!(s.starts_with("2024-"), "19723 天 = {s}");
    }
}
