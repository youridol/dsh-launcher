//! dsh-launcher 后端入口
//!
//! 模块划分见 docs/DESIGN.md：
//! - core: 进程/端口/配置/日志（Rust 重活）
//! - commands: Tauri IPC 命令（前端调用）

pub mod core;
pub mod commands;

use crate::core::logging::Logger;
use crate::core::process::ProcessManager;
use std::sync::Arc;

/// 全局状态：日志 + 进程管理器
pub struct AppState {
    pub logger: Arc<Logger>,
    pub process: Arc<ProcessManager>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let logger = Arc::new(Logger::init().expect("初始化日志失败"));
    let process = Arc::new(ProcessManager::new(Arc::clone(&logger)));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState { logger, process })
        .invoke_handler(tauri::generate_handler![
            commands::dsh::get_status,
            commands::dsh::start_dsh,
            commands::dsh::stop_dsh,
            commands::dsh::restart_dsh,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
