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
use tauri::Manager;

/// 全局状态：日志 + 进程管理器
pub struct AppState {
    pub logger: Arc<Logger>,
    pub process: Arc<ProcessManager>,
}

/// 打开内嵌 Web GUI 窗口（桌面快捷方式 --web-gui 触发时调用）
/// URL 从日志兜底提取（含 token 认证）；dsh 未运行时窗口加载会 401（可提示用户先启动）
fn open_web_gui_window(app: &tauri::AppHandle) {
    // 从最新日志提取带 token 的 web URL，失败回退裸 URL
    let url = crate::core::logging::extract_latest_web_url()
        .unwrap_or_else(|| "http://127.0.0.1:3080".to_string());
    let parsed = url.parse().unwrap_or_else(|_| "http://127.0.0.1:3080".parse().unwrap());
    let label = format!(
        "dsh-web-gui-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let _ = tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::External(parsed))
        .title("deepseek-harness Web UI")
        .inner_size(1600.0, 900.0)
        .build();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let logger = Arc::new(Logger::init().expect("初始化日志失败"));
    let process = Arc::new(ProcessManager::new(Arc::clone(&logger)));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // 单实例：只允许一个启动器进程（一个主窗口 + 一个托盘图标）
        // 二次启动/快捷方式触发时唤醒已有实例：聚焦主窗口；
        // 携带 --web-gui 时再开一个内嵌 Web GUI 窗口（不限制数量）
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
            if argv.iter().any(|a| a == "--web-gui") {
                open_web_gui_window(app);
            }
        }))
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
                // 设置主窗口/任务栏高清图标（512px PNG，DPI 缩放后仍清晰）
                if let Some(win) = app.get_webview_window("main") {
                    const WIN_ICON: &[u8] = include_bytes!("../icons/icon.png");
                    if let Ok(img) = tauri::image::Image::from_bytes(WIN_ICON) {
                        let _ = win.set_icon(img);
                    }
                }
                // 创建系统托盘
                core::tray::setup_tray(app.handle(), Arc::clone(&process), Arc::clone(&logger));
                // v0.3.5：启动时探测配置端口——dsh 已在运行（上次退出驻留）则恢复状态
                let cfg = crate::core::config::AppConfig::load();
                if cfg.port != 0 && crate::core::port::is_port_in_use(cfg.port) {
                    process.adopt_running(cfg.port);
                }
                // v0.3.2：桌面快捷方式（--web-gui 参数）→ 启动即打开内嵌 Web GUI 窗口
                if std::env::args().any(|a| a == "--web-gui") {
                    open_web_gui_window(app.handle());
                }
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
            commands::dsh::get_web_url,
            commands::dsh::create_desktop_shortcut,
            commands::dsh::start_dsh,
            commands::dsh::stop_dsh,
            commands::dsh::restart_dsh,
            commands::config::get_config,
            commands::config::set_port,
            commands::config::set_mirrors,
            commands::config::set_github_token,
            commands::config::set_switches,
            commands::toolchain::detect_toolchain,
            commands::toolchain::install_toolchain,
            commands::version::list_versions,
            commands::version::get_installed_version,
            commands::version::install_version,
            commands::version::get_install_paths,
            commands::version::uninstall,
            commands::logs::list_logs,
            commands::logs::read_log,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
