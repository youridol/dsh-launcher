//! 工具链检测与一键安装 IPC
//!
//! 清单（CONTEXT.md）：Node.js(22.19+/24+)、npm、pnpm、Git(2.26+)、Python(可选)
//! 状态：present / missing / mismatch
//! 安装：全部走官方通道、系统全局；需提权的步骤走 runas 子进程（Q22）

use serde::Serialize;
use std::process::Command;

/// 工具链项
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainItem {
    /// 名称
    pub name: &'static str,
    /// 已安装版本（未安装为 None）
    pub installed_version: Option<String>,
    /// 所需最低版本（用于提示）
    pub required: &'static str,
    /// 状态
    pub state: &'static str,
}

/// 检测所有工具链
#[tauri::command]
pub fn detect_toolchain() -> Vec<ToolchainItem> {
    vec![
        detect_node(),
        detect_npm(),
        detect_pnpm(),
        detect_git(),
        detect_python(),
    ]
}

/// 一键安装指定工具链（走官方通道）
#[tauri::command]
pub fn install_toolchain(name: String) -> Result<String, String> {
    match name.as_str() {
        "node" => install_node(),
        "git" => install_git(),
        "pnpm" => install_pnpm(),
        other => Err(format!("不支持的工具链: {other}")),
    }
}

fn detect_node() -> ToolchainItem {
    let ver = run_cmd("node", &["--version"]);
    ToolchainItem {
        name: "node",
        installed_version: ver.clone(),
        required: "22.19+ / 24+",
        state: if ver.is_some() { "present" } else { "missing" },
    }
}

fn detect_npm() -> ToolchainItem {
    let ver = run_cmd("npm", &["--version"]);
    ToolchainItem {
        name: "npm",
        installed_version: ver.clone(),
        required: "随 Node 提供",
        state: if ver.is_some() { "present" } else { "missing" },
    }
}

fn detect_pnpm() -> ToolchainItem {
    let ver = run_cmd("pnpm", &["--version"]);
    ToolchainItem {
        name: "pnpm",
        installed_version: ver.clone(),
        required: "任意（源码构建必需）",
        state: if ver.is_some() { "present" } else { "missing" },
    }
}

fn detect_git() -> ToolchainItem {
    let ver = run_cmd("git", &["--version"]);
    ToolchainItem {
        name: "git",
        installed_version: ver.clone(),
        required: "2.26+",
        state: if ver.is_some() { "present" } else { "missing" },
    }
}

fn detect_python() -> ToolchainItem {
    let ver = run_cmd("python", &["--version"]);
    ToolchainItem {
        name: "python",
        installed_version: ver.clone(),
        required: "3.10+（可选，SDK 用）",
        state: if ver.is_some() { "present" } else { "missing" },
    }
}

/// 运行命令取 stdout（首行）
fn run_cmd(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 安装 Node：从官方通道下载（占位实现，实际需走镜像源配置）
fn install_node() -> Result<String, String> {
    Err("Node 安装尚未实现：请配置镜像源后重试，或手动从 https://nodejs.org 下载".to_string())
}

/// 安装 Git：从官方通道下载（占位实现）
fn install_git() -> Result<String, String> {
    Err("Git 安装尚未实现：请配置镜像源后重试，或手动从 https://git-scm.com 下载".to_string())
}

/// 安装 pnpm：npm i -g pnpm
fn install_pnpm() -> Result<String, String> {
    let out = Command::new("npm")
        .args(["i", "-g", "pnpm"])
        .output()
        .map_err(|e| format!("npm 执行失败: {e}"))?;
    if out.status.success() {
        Ok("pnpm 安装成功".to_string())
    } else {
        Err(format!(
            "pnpm 安装失败: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}
