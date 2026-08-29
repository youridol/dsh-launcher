//! 配置读写 IPC：端口、镜像源、滑动开关
//!
//! 配置持久化在 `%APPDATA%\dsh-launcher\config.json`（core/config.rs）

use crate::core::config::AppConfig;
use serde::Serialize;

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
pub fn set_port(port: u16) -> Result<(), String> {
    if !(1..=65535).contains(&port) {
        return Err(format!("端口 {port} 非法"));
    }
    let mut cfg = AppConfig::load();
    cfg.port = port;
    cfg.save()
}

/// 保存 GitHub Token（防 API 限流 / git 认证增强）
#[tauri::command]
pub fn set_github_token(token: String) -> Result<(), String> {
    let mut cfg = AppConfig::load();
    cfg.github_token = token.trim().to_string();
    cfg.save()
}

/// 保存镜像源
#[tauri::command]
pub fn set_mirrors(
    npm_registry: String,
    github_mirror: String,
    node_mirror: String,
) -> Result<(), String> {
    let mut cfg = AppConfig::load();
    cfg.npm_registry = npm_registry;
    cfg.github_mirror = github_mirror;
    cfg.node_mirror = node_mirror;
    cfg.save()
}

/// 保存滑动开关
#[tauri::command]
pub fn set_switches(
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
    cfg.save()
}
