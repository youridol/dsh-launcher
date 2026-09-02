//! 配置读写 IPC：端口、镜像源、滑动开关
//!
//! 配置持久化在 `%APPDATA%\dsh-launcher\config.json`（core/config.rs）

use crate::core::config::AppConfig;
use serde::Serialize;
use tauri::State;

/// 完整配置视图（给前端）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigView {
    pub port: u16,
    pub npm_registry: String,
    pub github_mirror: String,
    pub github_token: String,
    pub node_mirror: String,
    pub close_exits: bool,
    pub minimize_to_tray: bool,
    pub keep_dsh_on_exit: bool,
    pub keep_dsh_home_on_uninstall: bool,
    pub auto_start_dsh: bool,
    pub auto_open_browser: bool,
}

impl From<AppConfig> for ConfigView {
    fn from(c: AppConfig) -> Self {
        Self {
            port: c.port,
            npm_registry: c.npm_registry,
            github_mirror: c.github_mirror,
            github_token: c.github_token,
            node_mirror: c.node_mirror,
            close_exits: c.close_exits,
            minimize_to_tray: c.minimize_to_tray,
            keep_dsh_on_exit: c.keep_dsh_on_exit,
            keep_dsh_home_on_uninstall: c.keep_dsh_home_on_uninstall,
            auto_start_dsh: c.auto_start_dsh,
            auto_open_browser: c.auto_open_browser,
        }
    }
}

/// 读取完整配置
#[tauri::command]
pub fn get_config() -> ConfigView {
    ConfigView::from(AppConfig::load())
}

/// 保存端口
#[tauri::command]
pub fn set_port(state: State<'_, crate::AppState>, port: u16) -> Result<(), String> {
    if !(1..=65535).contains(&port) {
        return Err(format!("端口 {port} 非法"));
    }
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
    let mut cfg = AppConfig::load();
    cfg.github_token = token.trim().to_string();
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
) -> Result<(), String> {
    let mut cfg = AppConfig::load();
    cfg.close_exits = close_exits;
    cfg.minimize_to_tray = minimize_to_tray;
    cfg.keep_dsh_on_exit = keep_dsh_on_exit;
    cfg.keep_dsh_home_on_uninstall = keep_dsh_home_on_uninstall;
    cfg.auto_start_dsh = auto_start_dsh;
    cfg.auto_open_browser = auto_open_browser;
    cfg.save()?;
    state.logger.info(&format!(
        "窗口与运行行为已保存（closeExits={close_exits}, minimizeToTray={minimize_to_tray}, keepDshOnExit={keep_dsh_on_exit}, keepDshHomeOnUninstall={keep_dsh_home_on_uninstall}, autoStartDsh={auto_start_dsh}, autoOpenBrowser={auto_open_browser}）"
    ));
    Ok(())
}
