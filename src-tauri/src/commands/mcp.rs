//! MCP Server 管理 IPC（ADR-0006 Part A）：列出 / 新增 / 删除 / 启停
//!
//! 薄壳：全部业务在 `core::mcp`，本模块只做 `spawn_blocking` 与事件广播
//! （与 `commands/plugin.rs` 同一约定）。
//!
//! **不获取进程 `op_lock`**：MCP 管理不涉及 bundle 成员变更，
//! 因此既不停止也不启动 dsh；`OpResult.restarted` 恒为 `false`。

use crate::core::mcp::{self, McpAddSpec, OpResult};
use crate::core::plugin::state::PluginError;
use crate::AppState;
use std::sync::Arc;
use tauri::State;

/// 列出合成树全量 MCP server（磁盘 + dump 为事实源）
#[tauri::command]
pub async fn mcp_list(state: State<'_, AppState>) -> Result<mcp::McpListResult, String> {
    let logger = Arc::clone(&state.logger);
    tauri::async_runtime::spawn_blocking(move || mcp::list(&logger))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))?
        .map_err(|e: PluginError| e.ipc_message())
}

/// 新增 MCP server（受管区块声明段 + 定向段）
#[tauri::command]
pub async fn mcp_add(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    spec: McpAddSpec,
) -> Result<OpResult, String> {
    let logger = Arc::clone(&state.logger);
    let result = tauri::async_runtime::spawn_blocking(move || mcp::add(&spec, &logger))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))?
        .map_err(|e: PluginError| e.ipc_message())?;
    crate::core::events::emit_mcp_changed(&app);
    Ok(result)
}

/// 删除 MCP server（`managed` 真删除；`external` 仅撤销定向覆盖）
#[tauri::command]
pub async fn mcp_remove(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    server_name: String,
) -> Result<OpResult, String> {
    let logger = Arc::clone(&state.logger);
    let result = tauri::async_runtime::spawn_blocking(move || mcp::remove(&server_name, &logger))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))?
        .map_err(|e: PluginError| e.ipc_message())?;
    crate::core::events::emit_mcp_changed(&app);
    Ok(result)
}

/// 启用 / 禁用 MCP server（只写定向行，`config` 绝不重渲染）
#[tauri::command]
pub async fn mcp_set_state(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    server_name: String,
    enabled: bool,
) -> Result<OpResult, String> {
    let logger = Arc::clone(&state.logger);
    let result = tauri::async_runtime::spawn_blocking(move || {
        mcp::set_enabled(&server_name, enabled, &logger)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e: PluginError| e.ipc_message())?;
    crate::core::events::emit_mcp_changed(&app);
    Ok(result)
}
