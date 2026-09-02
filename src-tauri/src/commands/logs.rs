//! 日志 IPC：日志文件列表、读取
//!
//! 日志落盘在 `%LOCALAPPDATA%\dsh-launcher\logs\<date>.log`（core/logging.rs）。
//! 前端实时流式通过 Tauri event `log://line` 推送（core/logging.rs emit）；
//! 本模块提供文件级列表/读取（补流与文件视图）。

use serde::Serialize;
use std::fs;

use crate::core::logging::logs_dir;

/// 日志文件条目
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFile {
    /// 相对路径（date.log 或 date/date.log）
    pub path: String,
    /// 大小（字节）
    pub size: u64,
    /// 修改时间（秒）
    pub modified: u64,
}

/// 列出所有日志文件（兼容两种布局：早期版本日期目录 logs/<date>/<date>.log，
/// 当前版本直接放根目录 logs/<date>.log）
#[tauri::command]
pub fn list_logs() -> Vec<LogFile> {
    let dir = logs_dir();
    let mut result = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return result;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_log = path.extension().map(|e| e == "log").unwrap_or(false);
        if path.is_dir() {
            // 早期布局：日期目录内的 .log 文件
            let Ok(files) = fs::read_dir(&path) else {
                continue;
            };
            for file in files.flatten() {
                let fpath = file.path();
                if fpath.extension().map(|e| e == "log").unwrap_or(false) {
                    if let Ok(meta) = fs::metadata(&fpath) {
                        result.push(LogFile {
                            path: format!(
                                "{}/{}",
                                entry.file_name().to_string_lossy(),
                                fpath.file_name().unwrap_or_default().to_string_lossy()
                            ),
                            size: meta.len(),
                            modified: meta
                                .modified()
                                .map(|m| {
                                    m.duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0)
                                })
                                .unwrap_or(0),
                        });
                    }
                }
            }
        } else if is_log {
            // 当前布局：根目录直接放 <date>.log
            if let Ok(meta) = fs::metadata(&path) {
                result.push(LogFile {
                    path: path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                    size: meta.len(),
                    modified: meta
                        .modified()
                        .map(|m| {
                            m.duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0)
                        })
                        .unwrap_or(0),
                });
            }
        }
    }
    result.sort_by_key(|f| std::cmp::Reverse(f.modified));
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
