//! 技能 IPC：列出 / 启用停用 / 删除（ADR-0007）
//!
//! 薄壳：全部业务在 `core::skill::{scan,manage}`。
//!
//! 写操作的成功路径会广播 `skill://changed`，前端据此重扫；
//! 幂等空操作（已是目标状态）**不广播**。

use crate::core::skill::{self, DeleteReport, SkillList, ToggleReport};
use crate::AppState;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

/// 列出全部受管技能（只读，与 dsh 运行状态无关）
#[tauri::command]
pub async fn skill_list() -> Result<SkillList, String> {
    tauri::async_runtime::spawn_blocking(skill::list)
        .await
        .map_err(|e| format!("任务执行失败: {e}"))
}

/// 启用/停用技能（单一开关 = `disable-model-invocation`）
///
/// `path` 与 `name` 是前端回传的**身份声明**：`name` 用于确认磁盘内容未被换掉，
/// `path` 用于确认目标仍在受管根内。二者不符即拒绝（ADR-0007 D10）。
#[tauri::command]
pub async fn skill_set_enabled(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    name: String,
    enabled: bool,
) -> Result<ToggleReport, String> {
    let logger = Arc::clone(&state.logger);
    let target = PathBuf::from(&path);
    let report = tauri::async_runtime::spawn_blocking(move || {
        skill::manage::set_enabled(&target, &name, enabled, &logger)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e| e.message())?;
    // 幂等空操作不广播（与插件的 BlockOutcome::Unchanged 纪律一致）
    if report.changed {
        crate::core::events::emit_skill_changed(&app);
    }
    Ok(report)
}

/// 删除技能（移入 `<root>/.trash/`，可恢复）
#[tauri::command]
pub async fn skill_delete(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    name: String,
) -> Result<DeleteReport, String> {
    let logger = Arc::clone(&state.logger);
    let target = PathBuf::from(&path);
    let report = tauri::async_runtime::spawn_blocking(move || {
        skill::manage::delete(&target, &name, &logger)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e| e.message())?;
    crate::core::events::emit_skill_changed(&app);
    Ok(report)
}

// ==================== ADR-0008：导入 / 检查更新 / 外部打开 ====================

/// 从任意 git URL 导入技能（克隆 → 递归扁平化 → 文件级覆盖；保留本地独有文件）
#[tauri::command]
pub async fn skill_import_url(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    url: String,
    apply: bool,
) -> Result<skill::import::ImportReport, String> {
    let logger = Arc::clone(&state.logger);
    let report = tauri::async_runtime::spawn_blocking(move || {
        skill::import::import_from_url(&url, None, apply, &logger)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e| e.message())?;
    if report.applied {
        crate::core::events::emit_skill_changed(&app);
    }
    Ok(report)
}

/// 批量导入：一次传入多个（仓库标签 + URL），逐条克隆 → 计划 → 可选应用。
/// **单条失败不中断其余条目**，返回逐条结果。
#[tauri::command]
pub async fn skill_import_batch(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    items: Vec<skill::import::ImportItem>,
    apply: bool,
) -> Result<skill::import::BatchImportReport, String> {
    let logger = Arc::clone(&state.logger);
    let report = tauri::async_runtime::spawn_blocking(move || {
        skill::import::import_batch(&items, apply, &logger)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?;
    if report.applied {
        crate::core::events::emit_skill_changed(&app);
    }
    Ok(report)
}

/// 检查已登记来源的技能更新（**纯只读，永不自动写盘**）
#[tauri::command]
pub async fn skill_check_updates(
    state: State<'_, AppState>,
) -> Result<skill::update::UpdateCheckReport, String> {
    let logger = Arc::clone(&state.logger);
    tauri::async_runtime::spawn_blocking(move || skill::update::check_updates(&logger))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))
}

/// 应用某个来源的更新（须由用户在界面上确认后调用）
#[tauri::command]
pub async fn skill_apply_update(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    url: String,
) -> Result<skill::import::ImportReport, String> {
    let logger = Arc::clone(&state.logger);
    let report = tauri::async_runtime::spawn_blocking(move || {
        skill::update::apply_source_update(&url, &logger)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e| e.message())?;
    crate::core::events::emit_skill_changed(&app);
    Ok(report)
}

/// 读取技能来源注册表（供 UI 显示来源与更新时间）
#[tauri::command]
pub fn skill_sources() -> skill::source::SourceRegistry {
    skill::source::SourceRegistry::load()
}

/// 移除某个来源记录（**只删元数据，不动技能文件**）
#[tauri::command]
pub fn skill_forget_source(state: State<'_, AppState>, url: String) -> Result<(), String> {
    let mut registry = skill::source::SourceRegistry::load();
    let before = registry.sources.len();
    registry.sources.retain(|s| s.url != url);
    if registry.sources.len() == before {
        return Err("未找到该来源记录".to_string());
    }
    registry.save()?;
    state
        .logger
        .info(&format!("已移除技能来源记录 {url}（技能文件未被删除）"));
    Ok(())
}

/// 用外部程序打开受管文件：闭集目标（agents-md / context-md / skills-root）
/// 或某个受管技能文件（路径经与写操作同款归属校验，绝不打开任意路径）
#[tauri::command]
pub async fn skill_open(
    state: State<'_, AppState>,
    target: Option<String>,
    path: Option<String>,
) -> Result<skill::editor::OpenReport, String> {
    let logger = Arc::clone(&state.logger);
    tauri::async_runtime::spawn_blocking(move || match (target, path) {
        // 受管技能文件：前端回传绝对路径，由 Rust 校验其仍属受管根
        (None, Some(path)) => {
            skill::editor::open_skill_file(std::path::Path::new(&path), &logger)
        }
        // 闭集目标：路径由 Rust 从 dshhome helper 推导
        (Some(target), None) => {
            let parsed = skill::editor::OpenTarget::parse(&target)?;
            skill::editor::open_target(parsed, &logger)
        }
        _ => Err(skill::editor::OpenError::UnknownTarget(
            "必须且只能提供 target 或 path 之一".to_string(),
        )),
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(|e| e.message())
}
