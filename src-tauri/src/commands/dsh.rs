//! dsh 生命周期 IPC 命令：启动 / 停止 / 重启 / 状态查询

use crate::core::process::DshStatus;
use crate::AppState;
use tauri::State;

/// 查询当前运行状态
#[tauri::command]
pub fn get_status(state: State<'_, AppState>) -> DshStatus {
    state.process.status()
}

/// 启动 dsh web（端口取自配置）
#[tauri::command]
pub fn start_dsh(state: State<'_, AppState>) -> Result<String, String> {
    let cfg = crate::core::config::AppConfig::load();
    let port = cfg.port;
    state.process.start(port)?;
    Ok(format!("dsh 已启动，端口 {port}"))
}

/// 停止 dsh
#[tauri::command]
pub fn stop_dsh(state: State<'_, AppState>) -> Result<String, String> {
    state.process.stop()?;
    Ok("dsh 已停止".to_string())
}

/// 重启 dsh（同端口同配置）
#[tauri::command]
pub fn restart_dsh(state: State<'_, AppState>) -> Result<String, String> {
    state.process.restart()?;
    Ok("dsh 已重启".to_string())
}
