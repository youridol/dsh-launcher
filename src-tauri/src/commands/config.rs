//! 配置读写 IPC：端口、镜像源、滑动开关
//!
//! 配置持久化在 `%APPDATA%\dsh-launcher\config.json`（core/config.rs）

use crate::core::config::AppConfig;
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

/// 配置读-改-写互斥锁（进程级单例）。
/// v0.4.13（审计修复 2.6）：set_* 命令此前各自 load→改→save 全量写回，
/// 并发保存（端口/镜像/开关）会互相覆盖丢失更新。统一在此串行化。
fn cfg_write_lock() -> std::sync::MutexGuard<'static, ()> {
    static CFG_MUTEX: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    CFG_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// 完整配置视图（给前端）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigView {
    pub port: u16,
    pub npm_registry: String,
    pub github_mirror: String,
    /// v0.4.13（审计修复 2.4）：不再回传明文 token，只回传“是否已设置”
    pub github_token_set: bool,
    pub node_mirror: String,
    pub close_exits: bool,
    pub minimize_to_tray: bool,
    pub keep_dsh_on_exit: bool,
    pub keep_dsh_home_on_uninstall: bool,
    pub auto_start_dsh: bool,
    pub auto_open_browser: bool,
    /// 启动后自动同步 upstream 插件
    pub auto_sync_plugins: bool,
    /// 外部编辑器命令（空 = 用系统默认关联程序打开，见 ADR-0008）
    pub editor_command: String,
    /// 是否已问过「用哪个程序打开」（首次点击编辑时弹一次引导）
    pub editor_prompt_seen: bool,
}

impl From<AppConfig> for ConfigView {
    fn from(c: AppConfig) -> Self {
        Self {
            port: c.port,
            npm_registry: c.npm_registry,
            github_mirror: c.github_mirror,
            github_token_set: !c.github_token.is_empty(),
            node_mirror: c.node_mirror,
            close_exits: c.close_exits,
            minimize_to_tray: c.minimize_to_tray,
            keep_dsh_on_exit: c.keep_dsh_on_exit,
            keep_dsh_home_on_uninstall: c.keep_dsh_home_on_uninstall,
            auto_start_dsh: c.auto_start_dsh,
            auto_open_browser: c.auto_open_browser,
            auto_sync_plugins: c.auto_sync_plugins,
            editor_command: c.editor_command,
            editor_prompt_seen: c.editor_prompt_seen,
        }
    }
}

/// 读取完整配置
///
/// G5（审计 RT-03）：改用 `load_checked()`，把「配置解析失败 / Token 解密失败」
/// 这类此前**静默**的回退事件落日志；不改变返回值形状（仍是 `ConfigView`）。
#[tauri::command]
pub fn get_config(state: State<'_, crate::AppState>) -> ConfigView {
    let (config, issue) = AppConfig::load_checked();
    if let Some(issue) = issue {
        state.logger.warn(&issue.message());
    }
    ConfigView::from(config)
}

/// 保存端口
#[tauri::command]
pub fn set_port(state: State<'_, crate::AppState>, port: u16) -> Result<(), String> {
    if !(1..=65535).contains(&port) {
        return Err(format!("端口 {port} 非法"));
    }
    let _guard = cfg_write_lock();
    let mut cfg = AppConfig::load();
    cfg.port = port;
    cfg.save()?;
    state.logger.info(&format!("端口已改为 {}", cfg.port));
    Ok(())
}

/// 保存 GitHub Token（防 API 限流 / git 认证增强）
/// token 内容不落日志（打码提示），避免敏感信息泄露
#[tauri::command]
pub fn set_github_token(state: State<'_, crate::AppState>, token: String) -> Result<(), String> {
    let token = token.trim().to_string();
    let _guard = cfg_write_lock();
    let mut cfg = AppConfig::load();
    // v0.4.13（审计修复 2.4）：空串 = 不修改（前端“留空保持原值”语义），
    // 避免旧 UI 把已保存 token 误清空。
    if token.is_empty() {
        return Ok(());
    }
    cfg.github_token = token;
    cfg.save()?;
    state
        .logger
        .info("GitHub Token 已保存（敏感信息不写入日志）");
    Ok(())
}

/// 保存镜像源
#[tauri::command]
pub fn set_mirrors(
    state: State<'_, crate::AppState>,
    npm_registry: String,
    github_mirror: String,
    node_mirror: String,
) -> Result<(), String> {
    let npm_disp = if npm_registry.is_empty() {
        "官方".to_string()
    } else {
        npm_registry.clone()
    };
    let gh_disp = if github_mirror.is_empty() {
        "官方".to_string()
    } else {
        github_mirror.clone()
    };
    let node_disp = if node_mirror.is_empty() {
        "官方".to_string()
    } else {
        node_mirror.clone()
    };
    let _guard = cfg_write_lock();
    let mut cfg = AppConfig::load();
    cfg.npm_registry = npm_registry;
    cfg.github_mirror = github_mirror;
    cfg.node_mirror = node_mirror;
    cfg.save()?;
    state.logger.info(&format!(
        "镜像源已更新（npm: {npm_disp} / github: {gh_disp} / node: {node_disp}）"
    ));
    Ok(())
}

/// 保存滑动开关
#[tauri::command]
pub fn set_switches(
    state: State<'_, crate::AppState>,
    close_exits: bool,
    minimize_to_tray: bool,
    keep_dsh_on_exit: bool,
    keep_dsh_home_on_uninstall: bool,
    auto_start_dsh: bool,
    auto_open_browser: bool,
    auto_sync_plugins: bool,
) -> Result<(), String> {
    let _guard = cfg_write_lock();
    let mut cfg = AppConfig::load();
    cfg.close_exits = close_exits;
    cfg.minimize_to_tray = minimize_to_tray;
    cfg.keep_dsh_on_exit = keep_dsh_on_exit;
    cfg.keep_dsh_home_on_uninstall = keep_dsh_home_on_uninstall;
    cfg.auto_start_dsh = auto_start_dsh;
    cfg.auto_open_browser = auto_open_browser;
    cfg.auto_sync_plugins = auto_sync_plugins;
    cfg.save()?;
    state.logger.info(&format!(
        "窗口与运行行为已保存（closeExits={close_exits}, minimizeToTray={minimize_to_tray}, keepDshOnExit={keep_dsh_on_exit}, keepDshHomeOnUninstall={keep_dsh_home_on_uninstall}, autoStartDsh={auto_start_dsh}, autoOpenBrowser={auto_open_browser}, autoSyncPlugins={auto_sync_plugins}）"
    ));
    Ok(())
}

/// 保存外部编辑器配置（ADR-0008）
///
/// `editor_command` 为空 = 使用系统默认关联程序。
/// `prompt_seen` 用于「首次点击编辑时弹一次引导」：引导完成后置 true，不再打扰。
#[tauri::command]
pub fn set_editor(
    state: State<'_, crate::AppState>,
    editor_command: String,
    prompt_seen: bool,
) -> Result<(), String> {
    let trimmed = editor_command.trim().to_string();
    let _guard = cfg_write_lock();
    let mut cfg = AppConfig::load();
    cfg.editor_command = trimmed.clone();
    cfg.editor_prompt_seen = prompt_seen;
    cfg.save()?;
    let display = if trimmed.is_empty() {
        "系统默认程序".to_string()
    } else {
        trimmed
    };
    state
        .logger
        .info(&format!("外部编辑器已设为 {display}"));
    Ok(())
}
