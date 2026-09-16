//! 插件管理 IPC：列表 / 安装 / 启停 / 卸载 / 同步 / 收敛
//!
//! 薄壳：全部业务在 `core::plugin`，本模块只做 spawn_blocking 与事件广播。

use crate::core::dshhome::MANAGED_PROFILE;
use crate::core::plugin::{self, state::PluginError, OpResult, PluginList, SyncReport};
use crate::core::plugin::spec::Origin;
use crate::AppState;
use std::sync::Arc;
use tauri::State;

/// 列出受管 profile 的插件（磁盘为事实源）
#[tauri::command]
pub async fn plugin_list(state: State<'_, AppState>) -> Result<PluginList, String> {
    let logger = Arc::clone(&state.logger);
    tauri::async_runtime::spawn_blocking(move || match plugin::list(MANAGED_PROFILE, &logger) {
        Ok(list) => Ok(list),
        Err(error) if error.kind == plugin::state::PluginErrorKind::NotFound => {
            // profile 尚未初始化：返回空列表并附提示（不算错误）
            Ok(PluginList {
                profile: MANAGED_PROFILE.to_string(),
                plugins: Vec::new(),
                degraded_reason: Some(error.message),
            })
        }
        Err(error) => Err(error.ipc_message()),
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 安装插件
#[tauri::command]
pub async fn plugin_install(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    spec: String,
    origin: Option<String>,
) -> Result<OpResult, String> {
    let logger = Arc::clone(&state.logger);
    let process = Arc::clone(&state.process);
    let origin_override = match origin.as_deref() {
        None | Some("") => None,
        Some("upstream") => Some(Origin::Upstream),
        Some("in-house") => Some(Origin::InHouse),
        Some("unknown") => Some(Origin::Unknown),
        Some(other) => return Err(format!("未知来源类型: {other}")),
    };
    let result = tauri::async_runtime::spawn_blocking(move || {
        plugin::install(MANAGED_PROFILE, &spec, origin_override, &logger, Some(&process))
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e: PluginError| e.ipc_message())?;
    crate::core::events::emit_plugin_changed(&app);
    Ok(result)
}

/// 启用/禁用插件（写受管区块，dsh 热重载）
#[tauri::command]
pub async fn plugin_set_state(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    package: String,
    enabled: bool,
) -> Result<OpResult, String> {
    let logger = Arc::clone(&state.logger);
    let result = tauri::async_runtime::spawn_blocking(move || {
        plugin::set_state(MANAGED_PROFILE, &package, enabled, &logger)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e: PluginError| e.ipc_message())?;
    crate::core::events::emit_plugin_changed(&app);
    Ok(result)
}

/// 修复 profile patch 配置文件（BUG-2，v0.9.8）。
///
/// 当 cordis.patch.yml 被外部工具写坏（重复拼接/顶层非法/marker 不成对）时，
/// dsh 无法启动且插件面板降级为只读。本命令把文件恢复成 dsh 可接受的最小合法
/// 形态：先备份原文件（`.corrupt-<ts>`，绝不静默丢弃数据），保留可提取的受管
/// 区块内容，再重建「模板头 + 受管区块」。
#[tauri::command]
pub async fn plugin_heal_config(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let logger = Arc::clone(&state.logger);
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let path = crate::core::dshhome::profile_patch_path(crate::core::dshhome::MANAGED_PROFILE)
            .map_err(|e| e)?;
        if !path.exists() {
            return Err(format!("{} 不存在，无需修复", path.display()));
        }
        // 体检：结构可信时不做无谓写入
        if plugin::inspect_patch_file(&path).is_none() {
            return Ok(format!("{} 结构正常，无需修复", path.display()));
        }
        let message = plugin::heal_patch_file(&path)?;
        logger.warn(&format!("已修复损坏的 profile patch：{message}"));
        Ok(message)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;
    crate::core::events::emit_plugin_changed(&app);
    Ok(result)
}

/// 卸载插件
#[tauri::command]
pub async fn plugin_uninstall(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    package: String,
) -> Result<OpResult, String> {
    let logger = Arc::clone(&state.logger);
    let process = Arc::clone(&state.process);
    let result = tauri::async_runtime::spawn_blocking(move || {
        plugin::uninstall(MANAGED_PROFILE, &package, &logger, Some(&process))
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e: PluginError| e.ipc_message())?;
    crate::core::events::emit_plugin_changed(&app);
    Ok(result)
}

/// 同步 upstream 插件（`apply=false` 只检查）
#[tauri::command]
pub async fn plugin_sync(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    apply: bool,
    package: Option<String>,
) -> Result<SyncReport, String> {
    let logger = Arc::clone(&state.logger);
    let process = Arc::clone(&state.process);
    let only = package.filter(|value| !value.is_empty());
    let result = tauri::async_runtime::spawn_blocking(move || {
        plugin::sync(MANAGED_PROFILE, apply, only.as_deref(), &logger, Some(&process))
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e: PluginError| e.ipc_message())?;
    if result.applied {
        crate::core::events::emit_plugin_changed(&app);
    }
    Ok(result)
}

/// 收敛：重新对账 bundles 并重放注册表期望态
#[tauri::command]
pub async fn plugin_repair(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    package: Option<String>,
) -> Result<OpResult, String> {
    let logger = Arc::clone(&state.logger);
    let process = Arc::clone(&state.process);
    let only = package.filter(|value| !value.is_empty());
    let result = tauri::async_runtime::spawn_blocking(move || {
        plugin::repair(MANAGED_PROFILE, only.as_deref(), &logger, Some(&process))
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e: PluginError| e.ipc_message())?;
    crate::core::events::emit_plugin_changed(&app);
    Ok(result)
}
