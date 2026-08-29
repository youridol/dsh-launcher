//! 工具链核心逻辑：Node/Git 下载、解压、安装（被 commands/toolchain.rs 调用）
//!
//! 策略（Q4/Q15/Q16 裁定）：
//! - Node：官方 zip 解压到 `%LOCALAPPDATA%\dsh-launcher\toolchain\node\`，用户级、免管理员
//! - Git：官方安装包（.exe），需提权 → 由 commands 层 spawn runas
//! - 镜像源：通过 AppConfig 注入下载 URL 前缀

use crate::core::config::AppConfig;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 工具链根目录：%LOCALAPPDATA%\dsh-launcher\toolchain
pub fn toolchain_dir() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("dsh-launcher")
        .join("toolchain")
}

/// Node 安装目录
pub fn node_dir() -> PathBuf {
    toolchain_dir().join("node")
}

/// 检查目录是否存在且有内容
pub fn dir_exists(p: &Path) -> bool {
    p.is_dir() && fs::read_dir(p).map(|mut it| it.next().is_some()).unwrap_or(false)
}

/// 下载文件到本地
/// base_url 可为镜像源；返回下载后本地路径
fn download(url: &str, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // 用 PowerShell 的 Invoke-WebRequest 下载（Windows 自带，无需额外依赖）
    let ps = format!(
        "Invoke-WebRequest -Uri '{}' -OutFile '{}' -UseBasicParsing",
        url.replace('\'', "''"),
        dest.to_string_lossy().replace('\'', "''")
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
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

/// 解压 zip 到目标目录（PowerShell Expand-Archive）
fn unzip(zip: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let ps = format!(
        "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
        zip.to_string_lossy().replace('\'', "''"),
        dest.to_string_lossy().replace('\'', "''")
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
        .output()
        .map_err(|e| format!("启动 PowerShell 解压失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "解压失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// 下载 Node zip 并解压到 node_dir（用户级，免管理员）
pub fn install_node() -> Result<String, String> {
    let cfg = AppConfig::load();
    // Node 官方 zip 地址；镜像源可通过配置覆盖
    let version = "v22.19.0"; // 满足 dsh 要求 22.19+；后续可做成可选项
    let file = format!("node-{version}-win-x64.zip");
    let base = if cfg.node_mirror.is_empty() {
        format!("https://nodejs.org/dist/{version}")
    } else {
        format!("{}/{}", cfg.node_mirror.trim_end_matches('/'), version)
    };
    let url = format!("{base}/{file}");

    let zip_path = toolchain_dir().join(&file);
    download(&url, &zip_path)?;

    // 解压到临时目录，再把 node 目录提取出来
    let tmp_dir = toolchain_dir().join("tmp-node");
    unzip(&zip_path, &tmp_dir)?;

    // 解压后结构：tmp/node-v22.19.0-win-x64/...
    let extracted = tmp_dir.join(format!("node-{version}-win-x64"));
    let node_install = node_dir();
    if node_install.exists() {
        fs::remove_dir_all(&node_install).map_err(|e| e.to_string())?;
    }
    if extracted.exists() {
        fs::rename(&extracted, &node_install).map_err(|e| e.to_string())?;
    } else {
        // 某些镜像解压结构不同，直接移动内容
        let mut found = false;
        if let Ok(entries) = fs::read_dir(&tmp_dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().contains("node-v") {
                    fs::rename(entry.path(), &node_install).map_err(|e| e.to_string())?;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return Err("解压后未找到 node 目录".to_string());
        }
    }
    let _ = fs::remove_dir_all(&tmp_dir);
    let _ = fs::remove_file(&zip_path);

    Ok(format!("Node {version} 已安装到 {}", node_install.display()))
}

/// 检查用户级 Node 是否可用（node_dir 里有 node.exe）
pub fn user_node_available() -> bool {
    node_dir().join("node.exe").exists()
}

/// 检查 Git 是否已安装（系统 PATH）
pub fn system_git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 清理下载/解压残留（启动器启动时可调用）
pub fn cleanup_temp() {
    let _ = fs::remove_dir_all(toolchain_dir().join("tmp-node"));
    let _ = fs::remove_dir_all(toolchain_dir().join("tmp-git"));
}
