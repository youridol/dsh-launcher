//! 系统托盘与窗口行为
//!
//! 语义（Q11/Q20 裁定）：
//! - 托盘菜单：打开主窗口 / 启动 dsh / 停止 dsh / 重启 dsh / 退出
//! - 滑动开关 1：主窗口关闭按钮 = 直接退出（含 dsh）还是最小化到托盘
//! - 滑动开关 2：最小化到托盘
//! - 退出：先优雅停止 dsh，再退出；可勾选"退出时驻留 dsh"
//!
//! 开关状态来自 AppConfig（core/config.rs）

use crate::core::config::AppConfig;
use crate::core::logging::{LogLevel, LogSource, Logger};
use crate::core::process::ProcessManager;
use std::sync::Arc;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, WindowEvent};

/// 托盘菜单项 ID
const MENU_SHOW: &str = "show";
const MENU_START: &str = "start";
const MENU_STOP: &str = "stop";
const MENU_RESTART: &str = "restart";
const MENU_QUIT: &str = "quit";

/// 构建系统托盘
pub fn setup_tray(app: &AppHandle, process: Arc<ProcessManager>, logger: Arc<Logger>) {
    let show = MenuItemBuilder::with_id(MENU_SHOW, "打开主窗口")
        .build(app)
        .expect("创建托盘菜单项失败");
    let start = MenuItemBuilder::with_id(MENU_START, "启动 dsh")
        .build(app)
        .expect("创建托盘菜单项失败");
    let stop = MenuItemBuilder::with_id(MENU_STOP, "停止 dsh")
        .build(app)
        .expect("创建托盘菜单项失败");
    let restart = MenuItemBuilder::with_id(MENU_RESTART, "重启 dsh")
        .build(app)
        .expect("创建托盘菜单项失败");
    let quit = MenuItemBuilder::with_id(MENU_QUIT, "退出")
        .build(app)
        .expect("创建托盘菜单项失败");

    let menu = MenuBuilder::new(app)
        .items(&[&show, &start, &stop, &restart, &quit])
        .build()
        .expect("构建托盘菜单失败");

    let mut tray_builder = TrayIconBuilder::with_id("dsh-launcher-tray")
        .tooltip("dsh-launcher")
        .menu(&menu)
        // 左键单击不弹菜单（改为唤起主窗口，见 on_tray_icon_event）；菜单右键弹出
        .show_menu_on_left_click(false);
    // 托盘图标：编译期嵌入 32x32 PNG（Windows 托盘标准尺寸），避免运行时路径丢失
    {
        const TRAY_ICON: &[u8] = include_bytes!("../../icons/32x32.png");
        if let Ok(img) = tauri::image::Image::from_bytes(TRAY_ICON) {
            tray_builder = tray_builder.icon(img);
        }
    }

    let _tray = tray_builder
        // 左键单击/双击托盘图标 → 唤起主窗口（恢复显示/还原/聚焦）
        .on_tray_icon_event(|tray, event| {
            let is_left_click = matches!(
                event,
                tauri::tray::TrayIconEvent::Click {
                    button: tauri::tray::MouseButton::Left,
                    button_state: tauri::tray::MouseButtonState::Up,
                    ..
                }
            ) || matches!(
                event,
                tauri::tray::TrayIconEvent::DoubleClick {
                    button: tauri::tray::MouseButton::Left,
                    ..
                }
            );
            if is_left_click {
                if let Some(win) = tray.app_handle().get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.unminimize();
                    let _ = win.set_focus();
                }
            }
        })
        .on_menu_event(move |app, event| {
            let id = event.id().as_ref();
            match id {
                MENU_SHOW => {
                    // 显示并聚焦主窗口
                    if let Some(win) = app.get_webview_window("main") {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }
                MENU_START => {
                    // 异步启动：不阻塞托盘事件循环
                    let p = Arc::clone(&process);
                    let logger = Arc::clone(&logger);
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = p.start(AppConfig::load().port) {
                            logger.log(
                                LogSource::Launcher,
                                LogLevel::Error,
                                &format!("托盘启动 dsh 失败: {e}"),
                            );
                        }
                    });
                }
                MENU_STOP => {
                    // 异步停止：stop 内部有 ≤5s 等待，必须放后台避免卡托盘
                    let p = Arc::clone(&process);
                    let logger = Arc::clone(&logger);
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = p.stop() {
                            logger.log(
                                LogSource::Launcher,
                                LogLevel::Error,
                                &format!("托盘停止 dsh 失败: {e}"),
                            );
                        }
                    });
                }
                MENU_RESTART => {
                    // 异步重启：内部含 stop(≤5s) + start
                    let p = Arc::clone(&process);
                    let logger = Arc::clone(&logger);
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = p.restart() {
                            logger.log(
                                LogSource::Launcher,
                                LogLevel::Error,
                                &format!("托盘重启 dsh 失败: {e}"),
                            );
                        }
                    });
                }
                MENU_QUIT => {
                    quit_app(app, process.clone());
                }
                _ => {}
            }
        })
        .build(app)
        .expect("创建系统托盘失败");
}

/// 退出应用：先优雅停止 dsh（除非配置"退出时驻留"），再退出
/// 异步执行：停止任务放后台线程，完成后退出应用（避免卡主线程）
pub fn quit_app(app: &AppHandle, process: Arc<ProcessManager>) {
    let cfg = AppConfig::load();
    if cfg.keep_dsh_on_exit {
        // 驻留 dsh，直接退出
        app.exit(0);
        return;
    }
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = process.stop();
        // 停止完成后退出应用
        app_handle.exit(0);
    });
}

/// 窗口事件处理：关闭/最小化按滑动开关行为
/// 主窗口 label（tauri.conf.json 未显式指定时 Tauri 默认 "main"，lib.rs 中亦用此 label）
const MAIN_WINDOW_LABEL: &str = "main";

/// 窗口事件处理：关闭/最小化按滑动开关行为
/// 仅主窗口应用关闭/最小化策略；内嵌 Web GUI 窗口（关闭/最小化）与启动器互不干涉
pub fn handle_window_event(
    window: &tauri::Window,
    event: &WindowEvent,
    process: Arc<ProcessManager>,
    logger: Arc<Logger>,
) {
    let cfg = AppConfig::load();
    // 非主窗口（内嵌 Web GUI 等）：关闭/最小化一律放行，不触发退出、不隐藏到托盘
    let is_main = window.label() == MAIN_WINDOW_LABEL;
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            if !is_main {
                // 内嵌窗口关闭：与启动器互不干涉，放行默认关闭（销毁窗口），不退出应用
                logger.log(
                    LogSource::Launcher,
                    LogLevel::Info,
                    &format!("内嵌窗口 ({}) 关闭 → 放行（与启动器互不干涉）", window.label()),
                );
                return;
            }
            if cfg.close_exits {
                // 开关 1 开：直接退出（含 dsh）——异步停止后退出，不卡窗口事件
                logger.log(LogSource::Launcher, LogLevel::Info, "关闭按钮 → 直接退出");
                api.prevent_close();
                let win = window.clone();
                // 预先 clone AppHandle（引用不能跨线程）
                let app_handle = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    let _ = process.stop();
                    // 停止完成后退出应用（退出应用会关闭所有窗口）
                    let _ = win.destroy();
                    app_handle.exit(0);
                });
            } else {
                // 开关 1 关：拦截关闭，最小化到托盘
                logger.log(
                    LogSource::Launcher,
                    LogLevel::Info,
                    "关闭按钮 → 最小化到托盘（dsh 继续运行）",
                );
                api.prevent_close();
                let _ = window.hide();
            }
        }
        WindowEvent::Resized(_) => {
            // 仅主窗口：最小化到托盘（开关 2）；内嵌窗口最小化不受影响
            if !is_main {
                return;
            }
            if cfg.minimize_to_tray && window.is_minimized().unwrap_or(false) {
                logger.log(
                    LogSource::Launcher,
                    LogLevel::Info,
                    "窗口最小化 → 隐藏到托盘",
                );
                let _ = window.hide();
            }
        }
        _ => {}
    }
}
