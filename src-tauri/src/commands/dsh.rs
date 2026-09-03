//! dsh 生命周期 IPC 命令：启动 / 停止 / 重启 / 状态查询 + 内嵌 Web GUI 窗口图标
//!
//! 并发策略（设计文档 docs/DESIGN.md §并发）：
//! - 所有命令 async（不占 Tauri IPC 线程）
//! - 阻塞操作（spawn/等待进程退出）经 spawn_blocking 跑在独立线程池
//! - 启动/停止/重启互不阻塞（各自独立任务）
//!
//! 图标：apply_window_icon 同时设置 SMALL（标题栏）与 ICON_BIG（任务栏高清，
//! 修复 WebView2 内嵌窗口任务栏图标模糊），见函数注释。

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
/// ERROR_INVALID_HANDLE（见取证测试 icon_forensics），而 CreateIcon(RGBA) 可靠。
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
    apply_window_icon(&win)
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
#[tauri::command]
pub async fn start_dsh(state: State<'_, AppState>) -> Result<String, String> {
    let cfg = crate::core::config::AppConfig::load();
    let port = cfg.port;
    let process = std::sync::Arc::clone(&state.process);
    // spawn_blocking：不阻塞 async 运行时
    tauri::async_runtime::spawn_blocking(move || {
        process.start(port)?;
        Ok::<_, String>(format!("dsh 已启动，端口 {port}"))
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
