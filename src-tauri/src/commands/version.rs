//! 版本与通道管理 IPC
//!
//! 双通道模型（ADR-0001）：
//! - npm 通道：registry 现有版本（当前最新 0.1.1-rc.2）
//! - GitHub 通道：v0.1.2-alpha.1 及更新源码 tag（core/github.rs）
//! 全局单版本（ADR-0003）：切换 = 先卸载再装

use crate::core::github as core_github;
use serde::Serialize;
use std::process::Command;

/// 通道枚举
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Npm,
    Github,
}

/// 一个可用版本
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshVersion {
    pub version: String,
    pub channel: Channel,
}

/// 获取某通道的可用版本列表
#[tauri::command]
pub fn list_versions(channel: String) -> Result<Vec<DshVersion>, String> {
    match channel.as_str() {
        "npm" => list_npm_versions(),
        "github" => list_github_versions(),
        other => Err(format!("未知通道: {other}")),
    }
}

/// 已安装的 dsh 版本（全局单版本）
#[tauri::command]
pub fn get_installed_version() -> Result<Option<String>, String> {
    let out = Command::new("dsh")
        .arg("--version")
        .output()
        .map_err(|e| format!("dsh 未安装或不可用: {e}"))?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(String::from_utf8(out.stdout)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty()))
}

/// 安装指定版本（npm 通道：npm i -g；GitHub 通道：clone + build）
#[tauri::command]
pub fn install_version(channel: String, version: String) -> Result<String, String> {
    match channel.as_str() {
        "npm" => install_npm_version(&version),
        "github" => install_github_version(&version),
        other => Err(format!("未知通道: {other}")),
    }
}

/// 卸载 dsh（npm 全局 + GitHub 通道目录）
#[tauri::command]
pub fn uninstall() -> Result<String, String> {
    // 1. 卸载 npm 全局包
    let _ = Command::new("npm")
        .args(["uninstall", "-g", "@deepseek-ai/dsh"])
        .output();
    // 2. 清理 GitHub 通道目录
    let gh_dir = core_github::github_dsh_dir();
    if gh_dir.exists() {
        std::fs::remove_dir_all(&gh_dir).map_err(|e| e.to_string())?;
    }
    Ok("dsh 已卸载（npm 全局包 + GitHub 源码目录）".to_string())
}

fn list_npm_versions() -> Result<Vec<DshVersion>, String> {
    let out = Command::new("npm")
        .args(["view", "@deepseek-ai/dsh", "versions", "--json"])
        .output()
        .map_err(|e| format!("npm view 执行失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "查询 npm 版本失败: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let versions: Vec<String> = serde_json::from_str(&text).unwrap_or_default();
    Ok(versions
        .into_iter()
        .map(|v| DshVersion {
            version: v,
            channel: Channel::Npm,
        })
        .collect())
}

fn list_github_versions() -> Result<Vec<DshVersion>, String> {
    let versions = core_github::list_releases()?;
    Ok(versions
        .into_iter()
        .map(|v| DshVersion {
            version: v,
            channel: Channel::Github,
        })
        .collect())
}

fn install_npm_version(version: &str) -> Result<String, String> {
    let spec = format!("@deepseek-ai/dsh@{version}");
    let out = Command::new("npm")
        .args(["install", "-g", &spec])
        .output()
        .map_err(|e| format!("npm 执行失败: {e}"))?;
    if out.status.success() {
        Ok(format!("已安装 dsh {version}（npm 通道）"))
    } else {
        Err(format!(
            "安装失败: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

fn install_github_version(version: &str) -> Result<String, String> {
    core_github::install_version(version)
}
