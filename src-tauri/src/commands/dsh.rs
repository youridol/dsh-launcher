//! dsh 生命周期 IPC 命令：启动 / 停止 / 重启 / 状态查询 + 内嵌 Web GUI 窗口图标
//!
//! 并发策略（设计文档 docs/DESIGN.md §并发）：
//! - 所有命令 async（不占 Tauri IPC 线程）
//! - 阻塞操作（spawn/等待进程退出）经 spawn_blocking 跑在独立线程池
//! - 启动/停止/重启互不阻塞（各自独立任务）
//!
//! 图标：apply_window_icon 同时设置 SMALL（标题栏）与 ICON_BIG（任务栏高清，
//! 修复 WebView2 内嵌窗口任务栏图标模糊），见函数注释。
//!
//! v0.4.15（审计修复）：内嵌窗口创建从"前端 new WebviewWindow + created 后置补发图标"
//! 收敛为单一 Rust 命令 create_web_gui_window —— 前端/快捷方式/自动打开三条入口共用
//! 同一带高清图标的 builder，消除"部分入口首帧缺失高清图标 → 任务栏模糊"的路径分歧
//! （详见 AUDIT_REPORT.md / FIX_PLAN_ICON.md）。

use crate::core::logging::{LogLevel, LogSource};
use crate::core::process::DshStatus;
use crate::AppState;
use tauri::{Manager, State};

/// 为窗口设置高清窗口图标（标题栏小图标 + 任务栏大图标）
///
/// 问题背景（WebView2 任务栏图标模糊修复）：
/// - Windows 任务栏按钮使用窗口的 ICON_BIG（32px 语义，实际按 DPI 取更大），
///   标题栏使用 ICON_SMALL（16px 语义）；
/// - tauri 的 `set_icon` 在 tao 层只发送 `ICON_SMALL`（见 tao set_window_icon），
///   任务栏按钮仍沿用默认 exe 资源图标经低分辨率缩放的模糊结果；
/// - 旧实现用 `CreateIconFromResourceEx` 从 icon.ico 构造 HICON 发 ICON_BIG，
///   但该 API 对本项目 PNG 压缩型 ICO（icon.ico 7 帧均为 PNG）全部失败
///   （实测 ERROR_INVALID_HANDLE）→ ICON_BIG 从未生效 → 任务栏模糊。
///
/// 本实现复刻 tao 的可靠路径：256px PNG → RGBA → `CreateIcon` 创建原生 HICON
/// （保留 256px 原生分辨率，Windows 高质量缩放到任意任务栏尺寸/DPI），
/// 同时发送 ICON_SMALL + ICON_BIG。HICON 由进程级静态缓存持有（不销毁），
/// 与应用同生命周期（WM_SETICON 后系统持有该句柄，立即销毁会导致重绘失效）。
///
/// v0.4.15（审计修复）：失败不再只落 stderr —— 调用方应同时经 Logger 记录一条
/// launcher 警告（release 无控制台，stderr 不可见；图标失败曾长期静默）。
pub(crate) fn apply_window_icon(win: &tauri::WebviewWindow) -> Result<(), String> {
    // 1. 小图标（标题栏 / Alt-Tab 小尺寸）：tauri set_icon（内部 RGBA → CreateIcon，可靠）
    const WIN_ICON: &[u8] = include_bytes!("../../icons/icon.png");
    let img = tauri::image::Image::from_bytes(WIN_ICON)
        .map_err(|e| format!("图标解析失败: {e}"))?;
    win.set_icon(img).map_err(|e| format!("设置图标失败: {e}"))?;

    // 2. 大图标（任务栏按钮 ICON_BIG）：256px 原生 HICON（进程级缓存，勿销毁）
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            SendMessageW, ICON_BIG, WM_SETICON,
        };

        let hwnd = win.hwnd().map_err(|e| format!("获取窗口句柄失败: {e}"))?;
        let hicon = ensure_big_hicon()?;
        // SAFETY: hwnd 为 tauri 提供的有效窗口句柄；hicon 为进程级缓存的有效 HICON，
        // 与应用同生命周期（不销毁）。SendMessageW 同步发送后系统持有图标副本。
        unsafe {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(hicon.0 as isize)),
            );
        }
    }
    Ok(())
}

/// 进程级缓存的 ICON_BIG HICON（256px 原生，应用生命周期内不销毁）
/// 存原始句柄值（usize）规避 HICON 非 Send：句柄跨线程传递安全（tao 同语义）
#[cfg(windows)]
static BIG_ICON_CACHE: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);

/// 懒创建并缓存 256px 原生 HICON（tao 同款路径：PNG → RGBA → CreateIcon）
///
/// 为何不用 CreateIconFromResourceEx：实测对本项目 PNG 压缩 ICO 全部返回
/// ERROR_INVALID_HANDLE（历史取证，见 tests/icon_window_test.rs 注释），
/// 而 CreateIcon(RGBA) 可靠。
#[cfg(windows)]
fn ensure_big_hicon() -> Result<windows::Win32::UI::WindowsAndMessaging::HICON, String> {
    use windows::Win32::UI::WindowsAndMessaging::CreateIcon;

    let mut guard = BIG_ICON_CACHE.lock().map_err(|_| "图标缓存锁失败".to_string())?;
    if let Some(raw) = *guard {
        // raw 由本函数 CreateIcon 创建并缓存，生命周期与应用一致
        return Ok(windows::Win32::UI::WindowsAndMessaging::HICON(
            raw as *mut core::ffi::c_void,
        ));
    }
    // 256px 源（128x128@2x.png），Windows 任务栏图标黄金尺寸：
    // 任何 DPI 下由系统从 256px 高质量缩放，杜绝低分辨率放大模糊
    const ICON_256: &[u8] = include_bytes!("../../icons/128x128@2x.png");
    let img = tauri::image::Image::from_bytes(ICON_256)
        .map_err(|e| format!("256px 图标解析失败: {e}"))?;
    let (w, h) = (img.width() as i32, img.height() as i32);
    let rgba = img.rgba();

    // 转 BGRA 并构造 AND mask（alpha 反转），与 tao RgbaIcon::into_windows_icon 相同
    let mut bgra = Vec::with_capacity(rgba.len());
    let mut and_mask = Vec::with_capacity((w * h) as usize);
    for px in rgba.chunks_exact(4) {
        bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        and_mask.push(px[3].wrapping_sub(u8::MAX));
    }

    // SAFETY: bgra/and_mask 为合法 BGRA/AND mask 缓冲（长度 = w*h*4 / w*h），
    // CreateIcon 同步复制数据后返回独立 HICON；调用方（缓存）负责其生命周期
    let hicon = unsafe {
        CreateIcon(None, w, h, 1, 32, and_mask.as_ptr(), bgra.as_ptr())
            .map_err(|e| format!("创建任务栏图标失败: {e}"))?
    };
    *guard = Some(hicon.0 as usize);
    Ok(hicon)
}

/// 为内嵌 Web GUI 窗口设置高清任务栏图标（前端创建的窗口创建完成后调用）
#[tauri::command]
pub fn set_web_gui_icon(app: tauri::AppHandle, label: String) -> Result<(), String> {
    let Some(win) = app.get_webview_window(&label) else {
        return Err(format!("窗口 {label} 不存在"));
    };
    apply_window_icon(&win).inspect_err(|e| {
        // v0.4.15（审计修复）：图标失败不再静默（release 无控制台，stderr 不可见）
        if let Some(state) = app.try_state::<AppState>() {
            state
                .logger
                .log(LogSource::Launcher, LogLevel::Warn, &format!("设置内嵌窗口 {label} 图标失败: {e}"));
        }
    })
}

/// 创建内嵌 Web GUI 窗口（统一实现；被桌面快捷方式/自动打开/前端"内嵌打开"共用）
///
/// 设计要点（任务栏图标高清修复，v0.4.15）：
/// - 返回 `Result<String, String>`（label），失败原因可落日志 / 返回前端提示——
///   此前 lib.rs build_web_gui_window 失败仅 eprintln（release 不可见）。
/// - 必须在**主线程**调用（Tauri 窗口非线程安全）：lib.rs 经 run_on_main_thread 调用；
///   前端 IPC 命令由 Tauri 主线程派发，天然满足。
/// - builder 预置 512px PNG（SMALL 首帧高清，消除"默认 exe 16px 图标"闪现），
///   创建后再 apply_window_icon 补发 SMALL+ICON_BIG（256px 原生 HICON）。
///
/// 链接放行设计（v0.4.14 修复外链 + v0.5.1 修复白屏）：
/// - dsh Web UI 会把 markdown 里的 http/https 外链渲染成 target="_blank" 锚点；
/// - opener 插件注入的点击拦截脚本会把这类点击转成 `plugin:opener|open_url` IPC，
///   该 IPC 已由 capabilities/dsh-web-gui.json 对该窗口放行 → 系统默认浏览器打开外链；
/// - 未经过 opener 脚本而到达 WebView2 原生 new-window 请求的（window.open / 部分
///   新窗口型链接）：**回环源（dsh UI 自身，127.0.0.1/localhost/::1）→ Allow 进程内
///   放行**（否则 dsh UI 内部 window.open 的界面全被掐 → 白屏，见 v0.5.1 修复）；
///   非回环 http(s) 外链 → 系统默认浏览器打开 + Deny（不产生游离 WebView2 子窗口）。
/// - 窗口内导航则全放行（本窗口仅作 dsh Web UI 的载体）。
pub(crate) fn create_embedded_web_gui_window(
    app: &tauri::AppHandle,
    url: &str,
) -> Result<String, String> {
    use tauri::Manager as _;

    let parsed: tauri::Url = url
        .parse()
        .unwrap_or_else(|_| "http://127.0.0.1:3080".parse().unwrap());
    // label 唯一化：毫秒时间戳低 32 位 + 进程内单调计数。
    // 同一毫秒内多次"内嵌打开"靠计数区分；跨进程实例由毫秒级时间戳区分，
    // 不会撞 label（tauri 对重 label 报 WindowLabelAlreadyExists）。
    let label = format!("dsh-web-gui-{}", unique_window_suffix());

    // 构造内嵌窗口。构建期预置 SMALL 图标（消除创建瞬间默认 exe 图标闪现）；
    // 图标源与主窗口一致（同一 512px PNG，同源生成）。
    // 解码/设置失败均不影响窗口创建：图标随后由 apply_window_icon 补发 SMALL+BIG（双保险）
    const WIN_ICON: &[u8] = include_bytes!("../../icons/icon.png");
    let builder = {
        let base = || {
            // 'static 闭包（on_new_window）需捕获 owned AppHandle，不能捕获函数内引用
            let app_owned = app.clone();
            let mut b = tauri::WebviewWindowBuilder::new(
                app,
                label.clone(),
                tauri::WebviewUrl::External(parsed.clone()),
            )
            .title("deepseek-harness Web UI")
            .inner_size(1600.0, 900.0)
            // 载体窗口：导航全放行（不拦截 dsh Web UI 内部任何跳转）
            .on_navigation(|_url| true);
            // 原生新窗口请求（window.open / 部分新窗口型链接）：
            // - 目标为 dsh Web UI 自身回环源（127.0.0.1 / localhost）→ Allow，进程内放行。
            //   （v0.5.1 修复：此前一律 Deny+系统浏览器会把 dsh UI 内部用 window.open
            //   打开的界面/面板全部掐掉 → 内嵌窗口白屏。0.4.14 前端 new WebviewWindow
            //   无此回调故正常，0.5.0 统一到 Rust builder 后回归暴露。）
            // - 其它 http(s) 外链 → 系统默认浏览器打开并 Deny（不让进程内长出游离子窗口）。
            // 回调运行在 wry 独立线程，禁止阻塞 UI（仅同步判断 + 打开浏览器）。
            b = b.on_new_window(move |url, _features| {
                let is_loopback = url
                    .host_str()
                    .map(|h| {
                        h.eq_ignore_ascii_case("127.0.0.1")
                            || h.eq_ignore_ascii_case("localhost")
                            || h.eq_ignore_ascii_case("::1")
                    })
                    .unwrap_or(false);
                if is_loopback {
                    return tauri::webview::NewWindowResponse::Allow;
                }
                if let Err(e) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>) {
                    // v0.5.2（日志全域化）：经 Logger 落盘（wry 线程调用，Logger 线程安全）
                    if let Some(state) = app_owned.try_state::<AppState>() {
                        state.logger.warn(&format!("系统浏览器打开外链失败 {url}: {e}"));
                    } else {
                        eprintln!("[dsh-launcher] 系统浏览器打开外链失败 {url}: {e}");
                    }
                }
                tauri::webview::NewWindowResponse::Deny
            });
            // v0.5.1 诊断注入（临时，DSH_WEBGUI_DIAG=1 时启用）：捕获页面 JS 错误存入
            // window.__dshDiag（含页面状态），Rust 侧用 eval_with_callback 轮询读回打日志，
            // 用于定位"内嵌白屏"根因。定位后移除。
            if std::env::var("DSH_WEBGUI_DIAG").is_ok() {
                const DIAG_SCRIPT: &str = r#"
                (function(){
                  try{
                    window.__dshDiag = { errors: [], readyState: document.readyState };
                    function push(m){ window.__dshDiag.errors.push(String(m).slice(0,200)); if(window.__dshDiag.errors.length>10) window.__dshDiag.errors.shift(); }
                    window.addEventListener('error', function(e){ push('ERR:'+(e.message||'')+' @'+(e.filename||'').split('/').pop()+':'+(e.lineno||'')); }, true);
                    window.addEventListener('unhandledrejection', function(e){ push('REJ:'+(e.reason&&e.reason.message||e.reason||'')); });
                    var origErr = console.error;
                    console.error = function(){ try{ push('CERR:'+Array.prototype.slice.call(arguments).map(function(a){try{return typeof a==='object'?JSON.stringify(a).slice(0,150):String(a)}catch(_){return String(a)}}).join(' ')); }catch(_){} return origErr.apply(console, arguments); };
                    var obs = new MutationObserver(function(){ window.__dshDiag.bodyLen = (document.body?document.body.innerHTML.length:-1); window.__dshDiag.readyState = document.readyState; });
                    obs.observe(document.documentElement, {childList:true, subtree:true, characterData:true});
                  }catch(e){ try{ window.__dshDiag = { fatal: String(e) }; }catch(_){} }
                })();
                "#;
                b = b.initialization_script(DIAG_SCRIPT);
            }
            b
        };
        match tauri::image::Image::from_bytes(WIN_ICON) {
            Ok(img) => match base().icon(img) {
                Ok(b) => b,
                Err(_) => base(), // 平台不支持预置时回退（罕见）
            },
            Err(_) => base(),
        }
    };
    let win = builder
        .build()
        .map_err(|e| format!("创建内嵌 Web GUI 窗口失败: {e}"))?;
    // 高清任务栏图标：SMALL + ICON_BIG（任务栏大图标，修复模糊）。
    // v0.5.2：图标设置失败**不阻断窗口创建**（0.4.14 语义）——
    // build() 后立即 hwnd() 在 WebView2 窗口未完全初始化时可能报
    // "the underlying handle is not available"（竞态），若 `?` 传播会把
    // 本已建成的窗口整体判失败 → 不开窗。失败仅记警告，窗口照常打开。
    if let Err(e) = apply_window_icon(&win) {
        if let Some(state) = app.try_state::<AppState>() {
            state.logger.log(
                LogSource::Launcher,
                LogLevel::Warn,
                &format!("设置内嵌 Web GUI 窗口 {label} 图标失败（不影响使用）: {e}"),
            );
        } else {
            eprintln!("[dsh-launcher] 设置内嵌窗口图标失败: {e}");
        }
    }
    // 诊断轮询（仅 DSH_WEBGUI_DIAG=1 时；定位白屏用，定位后随注入脚本一并移除）
    if std::env::var("DSH_WEBGUI_DIAG").is_ok() {
        let diag_win = win.clone();
        let diag_label = label.clone();
        std::thread::spawn(move || {
            use std::time::{Duration, Instant};
            let start = Instant::now();
            while Instant::now() - start < Duration::from_secs(90) {
                let js = "JSON.stringify({errors: (window.__dshDiag&&window.__dshDiag.errors)||[], readyState: document.readyState, bodyLen: (document.body?document.body.innerHTML.length:-1), url: location.href})";
                let js = js.to_string();
                let lab = diag_label.clone();
                let _ = diag_win.eval_with_callback(js, move |res| {
                    eprintln!("[dsh-diag {lab}] {res}");
                });
                std::thread::sleep(Duration::from_secs(3));
            }
        });
    }
    Ok(label)
}

/// 生成唯一窗口后缀（label 去重用）：毫秒时间戳低 32 位 + 进程内单调计数。
/// 两者拼接：同毫秒多次打开靠计数递增区分，跨进程靠时间戳区分。
/// 非密码学用途，避免引入 rand 依赖。
fn unique_window_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{ts:x}-{count:x}")
}

/// 前端"内嵌打开"统一入口（v0.4.15，审计修复）：
/// 前端拿到带 token 完整 URL 后调用本命令创建窗口，替代此前 `new WebviewWindow`
/// （JS 创建的窗口首帧无高清图标 → 任务栏模糊，见 FIX_PLAN_ICON.md）。
/// 窗口创建由 Tauri 主线程完成，图标在创建时同步设置 → 无"先默认图标后补发"窗口期。
///
/// v0.5.2（白屏/卡死修复）：**主线程建窗 + 不跨线程阻塞等待**。
/// 此前 sync fn 内部 `run_on_main_thread(建窗) + std::mpsc::recv()` 阻塞命令线程；
/// 若主线程正被 WebView2 同步窗口消息占用（内嵌页面加载时常见），主线程与等待它
/// 的命令线程交叉等待 → 整进程死锁（内嵌白屏且所有窗口无法关闭，需任务管理器）。
/// 现在：命令在 async 线程把建窗调度到主线程即返回；真正建窗在主线程同步完成，
/// 创建失败由 create_embedded_web_gui_window 内部落日志。前端无需 label（图标已
/// 在创建时设置，不再依赖 setWebGuiIcon 兜底回传）。
#[tauri::command]
pub async fn create_web_gui_window(app: tauri::AppHandle, url: String) -> Result<String, String> {
    // async command 在 async 运行时线程执行；窗口 API 要求主线程。
    // 调度到主线程建窗，不等待结果（无跨线程阻塞 → 无死锁）。
    let app2 = app.clone();
    app2.run_on_main_thread(move || {
        // 内部失败会落日志（create_embedded_web_gui_window 内 inspect_err + eprintln）
        let _ = create_embedded_web_gui_window(&app, &url);
    })
    .map_err(|e| format!("调度窗口创建到主线程失败: {e}"))?;
    Ok("已请求打开内嵌 Web GUI 窗口".to_string())
}

/// 查询当前运行状态（快，直接返回）
#[tauri::command]
pub fn get_status(state: State<'_, AppState>) -> DshStatus {
    state.process.status()
}

/// 获取 dsh web 完整访问 URL（含 token，供内嵌/外部打开免认证）
/// 内存中未捕获时，从最新日志文件兜底提取（dsh stdout 已实时落盘）
#[tauri::command]
pub fn get_web_url(state: State<'_, AppState>) -> String {
    let mem = state.process.web_url();
    if !mem.is_empty() {
        return mem;
    }
    // 兜底：扫描最新日志文件，提取带 token 的 URL
    crate::core::logging::extract_latest_web_url().unwrap_or_default()
}

/// 探测 dsh web 是否真正可服务（HTTP 200）。
/// v0.5.5（冷启动 404 修复）：端口监听/TCP 通 ≠ HTTP 路由就绪——冷启动时 dsh 先
/// 监端口后挂路由，期间带 token 请求返回 404，此时开窗 WebView2 首载命中 404 且
/// 不自动恢复（须关闭重开）。前端在拿到 token URL 后轮询本命令直到 true 再开窗。
#[tauri::command]
pub fn probe_web_ready(url: String) -> bool {
    crate::core::port::web_ready(&url, 2000)
}

/// 创建桌面快捷方式（.lnk，双击启动 dsh-launcher 并打开内嵌 Web GUI 窗口）
/// 注：URL 由启动器侧在收到 --web-gui 参数时重新解析（见 lib.rs），无需在此传入
#[tauri::command]
pub fn create_desktop_shortcut(state: State<'_, AppState>) -> Result<String, String> {
    // 当前 exe 路径
    let exe = std::env::current_exe().map_err(|e| format!("获取程序路径失败: {e}"))?;
    // 桌面路径：%USERPROFILE%\Desktop（Windows）
    let desktop = std::env::var("USERPROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("Desktop");
    std::fs::create_dir_all(&desktop).map_err(|e| format!("创建桌面目录失败: {e}"))?;

    let lnk = desktop.join("deepseek-harness.lnk");
    // 用 PowerShell 创建 .lnk（WScript.Shell），目标 = 本程序 + --web-gui 参数
    let ps = format!(
        "$ws=New-Object -ComObject WScript.Shell; $s=$ws.CreateShortcut('{}'); $s.TargetPath='{}'; $s.Arguments='--web-gui'; $s.IconLocation='{}'; $s.Save()",
        lnk.to_string_lossy().replace('\'', "''"),
        exe.to_string_lossy().replace('\'', "''"),
        exe.to_string_lossy().replace('\'', "''"),
    );
    let mut c = crate::core::command::hidden("powershell");
    c.args(["-NoProfile", "-Command", &ps]);
    let out = c
        .output()
        .map_err(|e| format!("启动 PowerShell 创建快捷方式失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "创建快捷方式失败: {}",
            crate::core::text::decode(&out.stderr).trim()
        ));
    }
    if !lnk.exists() {
        return Err("创建快捷方式失败（文件未生成）".to_string());
    }
    state
        .logger
        .info(&format!("已创建桌面快捷方式（打开内嵌窗口）: {}", lnk.display()));
    Ok(format!("已创建桌面快捷方式（打开内嵌窗口）: {}", lnk.display()))
}

/// 启动 dsh web（端口取自配置；阻塞操作放后台线程）
///
/// v0.5.1（产品优化）：启动后**等待端口就绪（≤READY_WAIT）再返回**——
/// 此前 spawn 即返回，前端只能看到"启动中"，要等 5s 轮询才转"运行中"，
/// 用户感知"点了启动没反应"。现在 await 返回时 dsh 已监听端口（状态 Running），
/// 前端 refresh 立即显示"已启动/运行中"。
#[tauri::command]
pub async fn start_dsh(state: State<'_, AppState>) -> Result<String, String> {
    let cfg = crate::core::config::AppConfig::load();
    let port = cfg.port;
    let process = std::sync::Arc::clone(&state.process);
    // spawn_blocking：不阻塞 async 运行时
    tauri::async_runtime::spawn_blocking(move || {
        process.start(port)?;
        // 等待端口就绪（探活线程亦会置 Running；此处同步等待使命令返回即"已就绪"）。
        // 注意：不依赖探活线程结果，直接轮询端口；失败（超时）不视为启动失败
        // （进程可能仍在预热/插件加载），返回时状态由后台探活线程继续收敛。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            if crate::core::port::is_port_in_use(port) {
                break;
            }
            // 进程已退出则不再等待（避免空等）：状态回 Stopped/Error 说明启动即崩
            if !matches!(
                process.status(),
                DshStatus::Starting | DshStatus::Running | DshStatus::Stopping
            ) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        if crate::core::port::is_port_in_use(port) {
            Ok::<_, String>(format!("dsh 已启动并就绪，端口 {port}"))
        } else {
            Ok::<_, String>(format!("dsh 已启动（端口 {port} 等待就绪中…）"))
        }
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 停止 dsh（阻塞操作放后台线程）
#[tauri::command]
pub async fn stop_dsh(state: State<'_, AppState>) -> Result<String, String> {
    let process = std::sync::Arc::clone(&state.process);
    tauri::async_runtime::spawn_blocking(move || {
        process.stop()?;
        Ok::<_, String>("dsh 已停止".to_string())
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 重启 dsh（同端口同配置；阻塞操作放后台线程）
#[tauri::command]
pub async fn restart_dsh(state: State<'_, AppState>) -> Result<String, String> {
    let process = std::sync::Arc::clone(&state.process);
    tauri::async_runtime::spawn_blocking(move || {
        process.restart()?;
        Ok::<_, String>("dsh 已重启".to_string())
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}
