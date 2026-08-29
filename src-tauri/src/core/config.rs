//! 启动器配置持久化
//!
//! 配置存于 `%APPDATA%\dsh-launcher\config.json`。
//! 字段语义见 CONTEXT.md 与 docs/DESIGN.md。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// 启动器全局配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    /// dsh web 启动端口（用户自定义，默认 3080）
    pub port: u16,
    /// npm registry 镜像源（空 = 官方源）
    pub npm_registry: String,
    /// GitHub 加速镜像（clone / release 下载，空 = 官方源）
    pub github_mirror: String,
    /// 主窗口关闭按钮行为：true = 直接退出（含 dsh），false = 最小化到托盘
    pub close_exits: bool,
    /// 最小化到托盘
    pub minimize_to_tray: bool,
    /// 退出时驻留 dsh（不停止）
    pub keep_dsh_on_exit: bool,
    /// 卸载时保留 DSH_HOME
    pub keep_dsh_home_on_uninstall: bool,
    /// 启动时自动启动 dsh
    pub auto_start_dsh: bool,
    /// 启动时自动打开浏览器
    pub auto_open_browser: bool,
}

impl AppConfig {
    /// 默认端口
    pub const DEFAULT_PORT: u16 = 3080;

    /// 计算配置文件的默认路径
    pub fn config_path() -> PathBuf {
        dirs_config_dir().join("dsh-launcher").join("config.json")
    }

    /// 读取配置；文件不存在时返回默认配置
    pub fn load() -> Self {
        let path = Self::config_path();
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// 写入配置
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
    }
}

/// 定位系统配置目录（Windows: %APPDATA%）
fn dirs_config_dir() -> PathBuf {
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}
