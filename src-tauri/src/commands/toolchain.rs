//! 工具链检测与一键安装 IPC
//!
//! 清单（CONTEXT.md）：Node.js(22.19+/24+)、npm、pnpm、Git(2.26+)、Python(可选)
//! 状态：present / missing / mismatch
//! 安装策略（Q4/Q15/Q16）：
//! - Node：官方 zip 解压到用户级目录（免管理员，见 core/toolchain.rs）
//! - Git：官方安装包，需提权 → spawn runas（Q22）
//! - pnpm：npm i -g

use crate::core::command;
use crate::core::toolchain as core_toolchain;
use serde::Serialize;

/// 工具链项
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainItem {
    pub name: &'static str,
    pub installed_version: Option<String>,
    pub required: &'static str,
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
    // 优先系统 PATH 的 node；若无，再看用户级目录
    let ver = run_cmd("node", &["--version"]).or_else(|| {
        let exe = core_toolchain::node_dir().join("node.exe");
        if exe.exists() {
            let mut c = command::hidden(&exe);
            c.arg("--version");
            c.output().ok().and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
                } else {
                    None
                }
            })
        } else {
            None
        }
    });
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
    let mut c = command::hidden(bin);
    c.args(args);
    let out = c.output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 安装 Node：官方 zip 解压到用户级目录（免管理员）
fn install_node() -> Result<String, String> {
    core_toolchain::install_node()
}

/// 安装 Git：官方安装包，需提权（runas）
fn install_git() -> Result<String, String> {
    // 下载 Git 官方安装包到本地
    let url = "https://github.com/git-for-windows/git/releases/download/v2.47.1.windows.1/Git-2.47.1-64-bit.exe";
    let dest = core_toolchain::toolchain_dir().join("Git-2.47.1-64-bit.exe");
    // 下载（用户级，无需提权）
    download_file(url, &dest)?;
    // 提权运行安装包（静默安装到 Program Files）
    // runas 需要管理员；通过 PowerShell Start-Process -Verb RunAs 触发 UAC
    let ps = format!(
        "Start-Process -FilePath '{}' -ArgumentList '/VERYSILENT','/NORESTART' -Verb RunAs",
        dest.to_string_lossy().replace('\'', "''")
    );
    let mut c = command::hidden("powershell");
    c.args(["-NoProfile", "-Command", &ps]);
    let out = c
        .output()
        .map_err(|e| format!("启动 Git 安装失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Git 安装启动失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(format!(
        "Git 安装程序已启动（UAC 确认后完成），安装包位于 {}",
        dest.display()
    ))
}

/// 下载文件到本地（复用 PowerShell）
fn download_file(url: &str, dest: &std::path::Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let ps = format!(
        "Invoke-WebRequest -Uri '{}' -OutFile '{}' -UseBasicParsing",
        url.replace('\'', "''"),
        dest.to_string_lossy().replace('\'', "''")
    );
    let mut c = command::hidden("powershell");
    c.args(["-NoProfile", "-Command", &ps]);
    let out = c
        .output()
        .map_err(|e| format!("启动 PowerShell 下载失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "下载失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// 安装 pnpm：npm i -g pnpm
fn install_pnpm() -> Result<String, String> {
    let mut c = command::hidden("npm");
    c.args(["i", "-g", "pnpm"]);
    let out = c
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
