//! dsh-launcher 后端入口
//!
//! 模块划分见 docs/DESIGN.md：
//! - core: 进程/端口/配置/日志/工具链/GitHub/托盘（Rust 重活）
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
        .manage(AppState {
            logger: Arc::clone(&logger),
            process: Arc::clone(&process),
        })
        .setup({
            let process = Arc::clone(&process);
            let logger = Arc::clone(&logger);
            move |app| {
                // 关联日志发射器（前端实时流式接收）
                logger.set_emitter(app.handle().clone());
                // 创建系统托盘
                core::tray::setup_tray(app.handle(), process, logger);
                Ok(())
            }
        })
        .on_window_event({
            let process = Arc::clone(&process);
            let logger = Arc::clone(&logger);
            move |window, event| {
                core::tray::handle_window_event(window, event, Arc::clone(&process), Arc::clone(&logger));
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::dsh::get_status,
            commands::dsh::start_dsh,
            commands::dsh::stop_dsh,
            commands::dsh::restart_dsh,
            commands::config::get_config,
            commands::config::set_port,
            commands::config::set_mirrors,
            commands::config::set_switches,
            commands::toolchain::detect_toolchain,
            commands::toolchain::install_toolchain,
            commands::version::list_versions,
            commands::version::get_installed_version,
            commands::version::install_version,
            commands::version::uninstall,
            commands::logs::list_logs,
            commands::logs::read_log,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
