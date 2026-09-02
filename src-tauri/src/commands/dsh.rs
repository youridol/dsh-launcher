//! dsh 生命周期 IPC 命令：启动 / 停止 / 重启 / 状态查询
//!
//! 并发策略（AGENTS.md 六、并发）：
//! - 所有命令 async（不占 Tauri IPC 线程）
//! - 阻塞操作（spawn/等待进程退出）经 spawn_blocking 跑在独立线程池
//! - 启动/停止/重启互不阻塞（各自独立任务）

use crate::core::process::DshStatus;
use crate::AppState;
use tauri::{Manager, State};

/// 为窗口设置高清任务栏图标（SMALL 小图标 + Windows ICON_BIG 大图标）
/// 修复：Windows 任务栏按钮使用 ICON_BIG（32px 大图标），而 tauri `set_icon` 仅设置
/// ICON_SMALL（16px 缩放）→ 任务栏图标模糊。这里补充从多尺寸 icon.ico 提取 32px 设 ICON_BIG。
pub(crate) fn apply_window_icon(win: &tauri::WebviewWindow) -> Result<(), String> {
    // 1. 小图标（标题栏/Alt-Tab 小尺寸）：tauri set_icon（512px PNG → Windows 缩放）
    const WIN_ICON: &[u8] = include_bytes!("../../icons/icon.png");
    let img = tauri::image::Image::from_bytes(WIN_ICON)
        .map_err(|e| format!("图标解析失败: {e}"))?;
    win.set_icon(img).map_err(|e| format!("设置图标失败: {e}"))?;

    // 2. 大图标（任务栏按钮 ICON_BIG）：从多尺寸 icon.ico 内存创建 32px HICON 并发送
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateIconFromResourceEx, DestroyIcon, SendMessageW, ICON_BIG, LR_DEFAULTSIZE,
            WM_SETICON,
        };

        let hwnd = win.hwnd().map_err(|e| format!("获取窗口句柄失败: {e}"))?;
        const ICO_BYTES: &[u8] = include_bytes!("../../icons/icon.ico");
        // SAFETY: icon.ico 编译期嵌入且为合法多尺寸 ICO；CreateIconFromResourceEx 返回独立 HICON
        let hicon = unsafe {
            CreateIconFromResourceEx(
                ICO_BYTES,
                true,
                0x0003_0000, // 图标资源版本（支持 Vista+ PNG 压缩 ICO）
                32,
                32,
                LR_DEFAULTSIZE,
            )
        };
        if let Ok(hicon) = hicon {
            // SAFETY: hwnd 为 tauri 提供的有效窗口句柄；SendMessageW 同步发送后系统持有图标
            unsafe {
                let _ = SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(ICON_BIG as usize)),
                    Some(LPARAM(hicon.0 as isize)),
                );
            }
            // 发送后系统已复制图标句柄，销毁本地引用防泄漏
            unsafe { let _ = DestroyIcon(hicon); }
        } else {
            return Err(format!("创建任务栏图标失败: {:?}", hicon.err()));
        }
    }
    Ok(())
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
#[tauri::command]
pub fn create_desktop_shortcut(state: State<'_, AppState>) -> Result<String, String> {
    // 确认 dsh web 已运行（快捷方式打开的仍是内嵌窗口，需 dsh 在运行）
    let url = state.process.web_url();
    let _ = url; // URL 由启动器侧在 --web-gui 启动时重新解析

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
            String::from_utf8_lossy(&out.stderr).trim()
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
