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

/// 打开内嵌 Web GUI 窗口（桌面快捷方式 --web-gui / 自动打开 / 托盘唤醒时调用）
///
/// 分发机器修复（v0.4.4）：
/// - 不再"立刻打开一个很可能打不开/未认证的窗口"：
///   dsh 冷启动时端口可能尚未监听（窗口显示 ERR_CONNECTION_REFUSED），
///   token URL 可能要等 dsh web 就绪后数秒才输出（过早打开 → 裸 URL → 401 认证页）。
/// - 本函数放到后台线程：先等端口监听（≤8s），再等带 token 的完整 URL（≤10s），
///   拿到才开窗；dsh 未运行/始终无 URL 时聚焦主窗口并落日志提示，不再开死窗口。
/// - 窗口创建仍回主线程（Tauri 窗口非线程安全），见 run_on_main_thread。
fn open_web_gui_window(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        use std::time::{Duration, Instant};

        let logger = app.state::<AppState>().logger.clone();
        let process: Arc<ProcessManager> = app.state::<AppState>().process.clone();

        // 目标端口：配置端口（旧配置可能存 0，兜底 3080）
        let cfg_port = crate::core::config::AppConfig::load().port;
        let port = if crate::core::port::validate_port(cfg_port) { cfg_port } else { 3080 };

        // ① 等端口监听（dsh 冷启动 / 直接 node 预热需数秒）
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut listening = false;
        while Instant::now() < deadline {
            if crate::core::port::probe(port) == Some(true) {
                listening = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(400));
        }

        if !listening {
            // dsh 未在运行/未就绪：不开"连接被拒绝"的窗口，聚焦主窗口让用户先启动
            logger.log(
                crate::core::logging::LogSource::Launcher,
                crate::core::logging::LogLevel::Warn,
                &format!("打开 Web GUI：dsh 未监听端口 {port}（可能未启动），请在启动器中先启动 dsh"),
            );
            let app2 = app.clone();
            let app2_inner = app2.clone();
            let _ = app2.run_on_main_thread(move || {
                if let Some(win) = app2_inner.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.unminimize();
                    let _ = win.set_focus();
                }
            });
            return;
        }

        // ② 等带 token 的完整 URL（内存捕获优先，日志兜底；≤10s）
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut url: Option<String> = None;
        while Instant::now() < deadline {
            let mem = process.web_url();
            if !mem.is_empty() {
                url = Some(mem);
                break;
            }
            if let Some(u) = crate::core::logging::extract_latest_web_url() {
                url = Some(u);
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let url = url.unwrap_or_else(|| format!("http://127.0.0.1:{port}"));

        // ③ 主线程创建内嵌窗口（带高清图标）
        let app3 = app.clone();
        let app3_inner = app3.clone();
        let _ = app3.run_on_main_thread(move || build_web_gui_window(&app3_inner, &url));
    });
}

/// 主线程上真正创建内嵌 Web GUI 窗口（label 唯一化 + 图标双保险）
///
/// 链接放行设计（v0.4.14，修复内嵌窗口无法打开会话超链接）：
/// - dsh Web UI 会把 markdown 里的 http/https 外链渲染成 target="_blank" 锚点；
/// - opener 插件注入的点击拦截脚本会把这类点击转成 `plugin:opener|open_url` IPC，
///   该 IPC 已由 capabilities/dsh-web-gui.json 对该窗口放行 → 系统默认浏览器打开外链；
/// - 若某次点击未经 opener 脚本（如 window.open、拖拽、快捷键直达）而到达 WebView2
///   原生 new-window 请求，wry 默认会直接取消（SetHandled(true)）——这里显式接管：
///   任何新窗口请求一律用系统默认浏览器打开目标 URL 并 Deny（不让 WebView2 在
///   启动器进程内长出游离子窗口），导航则全放行（本窗口仅作 dsh Web UI 的载体）。
fn build_web_gui_window(app: &tauri::AppHandle, url: &str) {
    let parsed: tauri::Url = url
        .parse()
        .unwrap_or_else(|_| "http://127.0.0.1:3080".parse().unwrap());
    let label = format!(
        "dsh-web-gui-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    // 构造内嵌窗口。构建期预置 SMALL 图标（消除创建瞬间默认 exe 图标闪现）；
    // 图标源与主窗口一致（同一 512px PNG，同源生成）。
    // 解码/设置失败均不影响窗口创建：图标随后由 apply_window_icon 补发 SMALL+BIG（双保险）
    const WIN_ICON: &[u8] = include_bytes!("../icons/icon.png");
    let builder = {
        let base = || {
            tauri::WebviewWindowBuilder::new(
                app,
                label.clone(),
                tauri::WebviewUrl::External(parsed.clone()),
            )
            .title("deepseek-harness Web UI")
            .inner_size(1600.0, 900.0)
            // 载体窗口：导航全放行（不拦截 dsh Web UI 内部任何跳转）
            .on_navigation(|_url| true)
            // 原生新窗口请求（window.open / 部分新窗口型链接）：一律丢给
            // 系统默认浏览器打开，并拒绝在进程内创建游离 WebView2 子窗口
            .on_new_window(|url, _features| {
                // 借用 app 打开外链；失败仅落 stderr，不阻断（见 wry 线程要求：
                // Windows 上该回调运行在独立线程，这里不做任何阻塞 UI 操作）
                if let Err(e) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>) {
                    eprintln!("[dsh-launcher] 系统浏览器打开外链失败 {url}: {e}");
                }
                tauri::webview::NewWindowResponse::Deny
            })
        };
        match tauri::image::Image::from_bytes(WIN_ICON) {
            Ok(img) => match base().icon(img) {
                Ok(b) => b,
                Err(_) => base(), // 平台不支持预置时回退（罕见）
            },
            Err(_) => base(),
        }
    };
    if let Ok(win) = builder.build() {
        // 高清任务栏图标：SMALL + ICON_BIG（任务栏大图标，修复模糊）
        // 启动期 logger 尚未关联 emitter，失败落 stderr 便于排查（错误已含 API/原因）
        if let Err(e) = commands::dsh::apply_window_icon(&win) {
            eprintln!("[dsh-launcher] 设置内嵌窗口图标失败: {e}");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // v0.4.13（审计修复 2.9）：Logger::init 内部已带目录降级，不再 expect panic
    let logger = Arc::new(Logger::init());
    let process = Arc::new(ProcessManager::new(Arc::clone(&logger)));
    // v0.4.13（审计修复 2.11）：后端 5s 状态对账（收养实例退出/外部启停收敛）
    process.spawn_reconcile();

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
                // 设置主窗口/任务栏高清图标（SMALL 512px PNG + ICON_BIG 256px，DPI 缩放后仍清晰）
                // 失败不阻断启动，落 stderr（错误已含 API/原因，便于排查）
                if let Some(win) = app.get_webview_window("main") {
                    if let Err(e) = commands::dsh::apply_window_icon(&win) {
                        eprintln!("[dsh-launcher] 设置主窗口图标失败: {e}");
                    }
                }
                // 创建系统托盘
                core::tray::setup_tray(app.handle(), Arc::clone(&process), Arc::clone(&logger));
                // v0.3.5：启动时探测配置端口——dsh 已在运行（上次退出驻留）则恢复状态
                // v0.4.13（审计修复 2.2）：adopt_running 内部先校验监听进程是否形如
                // dsh；端口被非 dsh 进程占用时返回 false 且仅落警告（不标记 Running）。
                let cfg = crate::core::config::AppConfig::load();
                if cfg.port != 0 && crate::core::port::is_port_in_use(cfg.port) {
                    process.adopt_running(cfg.port);
                }
                // 自动启动开关（SettingsPanel）：启动器启动时若 dsh 未在运行则自动拉起；
                // 启动成功且 autoOpenBrowser 开启 → 自动打开内嵌 Web GUI。
                // 放后台线程延迟执行（不阻塞 setup / 主窗口首帧），完成后由端口探活确认就绪。
                if cfg.auto_start_dsh
                    && cfg.port != 0
                    && !crate::core::port::is_port_in_use(cfg.port)
                {
                    let process = Arc::clone(&process);
                    let logger = Arc::clone(&logger);
                    let app_handle = app.handle().clone();
                    let open_gui = cfg.auto_open_browser;
                    // 后台线程延迟执行：不阻塞 setup / 主窗口首帧；
                    // 启动（含 spawn 阻塞）放独立线程，避免占用 async 运行时
                    std::thread::spawn(move || {
                        // 延迟 1.5s：等主窗口/托盘就绪后再启动，避免与启动期探测竞争
                        std::thread::sleep(std::time::Duration::from_millis(1500));
                        let port = crate::core::config::AppConfig::load().port;
                        if let Err(e) = process.start(port) {
                            logger.log(
                                crate::core::logging::LogSource::Launcher,
                                crate::core::logging::LogLevel::Error,
                                &format!("自动启动 dsh 失败: {e}"),
                            );
                            return;
                        }
                        logger.log(
                            crate::core::logging::LogSource::Launcher,
                            crate::core::logging::LogLevel::Info,
                            &format!("自动启动 dsh（autoStartDsh），端口 {port}"),
                        );
                        if open_gui {
                            // 自动打开内嵌 Web GUI：open_web_gui_window 内部会
                            // 等端口监听 + 等带 token 的完整 URL，拿到才开窗
                            // （分发机器修复：不再在 dsh 未就绪时打开 → 401/连接拒绝）
                            open_web_gui_window(&app_handle);
                        }
                    });
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
            commands::dsh::set_web_gui_icon,
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
            commands::toolchain::uninstall_toolchain,
            commands::toolchain::batch_install_toolchains,
            commands::toolchain::batch_uninstall_toolchains,
            commands::version::list_versions,
            commands::version::get_installed_version,
            commands::version::install_version,
            commands::version::get_install_paths,
            commands::version::uninstall,
            commands::logs::list_logs,
            commands::logs::read_log,
        ])
        .run(tauri::generate_context!())
        // v0.4.13（审计修复 2.9）：release 无控制台时 panic 不可见，改为
        // 显式落错误并退出（错误码 1），便于日志/进程退出码排查。
        .unwrap_or_else(|e| {
            eprintln!("[dsh-launcher] Tauri 应用运行失败: {e}");
            std::process::exit(1);
        });
}
