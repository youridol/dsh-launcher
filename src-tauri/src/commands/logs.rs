//! 日志 IPC：日志文件列表、导出、读取
//!
//! 日志落盘在 `%LOCALAPPDATA%\dsh-launcher\logs\<date>\<date>.log`（core/logging.rs）。
//! 前端实时流式通过 Tauri event 推送（后续接入），这里提供文件级读取/导出。

use serde::Serialize;
use std::fs;
use std::path::PathBuf;

/// 日志文件条目
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFile {
    /// 相对路径（date/file）
    pub path: String,
    /// 大小（字节）
    pub size: u64,
    /// 修改时间（秒）
    pub modified: u64,
}

/// 日志根目录
fn logs_dir() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("dsh-launcher")
        .join("logs")
}

/// 列出所有日志文件（按日期目录）
#[tauri::command]
pub fn list_logs() -> Vec<LogFile> {
    let dir = logs_dir();
    let mut result = Vec::new();
    let Ok(dates) = fs::read_dir(&dir) else {
        return result;
    };
    for date in dates.flatten() {
        let date_path = date.path();
        if !date_path.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(&date_path) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().map(|e| e == "log").unwrap_or(false) {
                if let Ok(meta) = fs::metadata(&path) {
                    result.push(LogFile {
                        path: format!(
                            "{}/{}",
                            date.file_name().to_string_lossy(),
                            path.file_name().unwrap_or_default().to_string_lossy()
                        ),
                        size: meta.len(),
                        modified: meta
                            .modified()
                            .map(|m| m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
                            .unwrap_or(0),
                    });
                }
            }
        }
    }
    result.sort_by(|a, b| b.modified.cmp(&a.modified));
    result
}

/// 读取指定日志文件内容（供前端展示/导出）
#[tauri::command]
pub fn read_log(rel_path: String) -> Result<String, String> {
    let base = logs_dir();
    // 防路径穿越：规范化后必须仍在 logs_dir 内
    let full = base.join(&rel_path);
    let canonical_base = base.canonicalize().map_err(|e| e.to_string())?;
    let canonical_full = full.canonicalize().map_err(|e| e.to_string())?;
    if !canonical_full.starts_with(&canonical_base) {
        return Err("非法路径".to_string());
    }
    fs::read_to_string(&full).map_err(|e| e.to_string())
}
