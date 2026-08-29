//! GitHub 通道核心逻辑：查询 release、clone 源码、pnpm 构建
//!
//! 双通道模型（ADR-0001）：GitHub 通道装 v0.1.2-alpha.1 及更新的源码 tag。
//! 全局单版本（ADR-0003）：当前激活版本 = 最近一次安装的版本。
//! 构建产物放 `%LOCALAPPDATA%\dsh-launcher\github-dsh\<version>\`，dsh bin 由 npm 全局链接。

use crate::core::command;
use crate::core::config::AppConfig;
use std::fs;
use std::path::PathBuf;

/// GitHub 仓库
const REPO: &str = "deepseek-ai/deepseek-harness";

/// GitHub 通道源码目录：%LOCALAPPDATA%\dsh-launcher\github-dsh\<version>
pub fn github_dsh_dir() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("dsh-launcher")
        .join("github-dsh")
}

/// 查询 GitHub releases（含镜像支持）
/// 返回版本号列表（降序，最新的在前）
pub fn list_releases() -> Result<Vec<String>, String> {
    let cfg = AppConfig::load();
    let api_base = if cfg.github_mirror.is_empty() {
        format!("https://api.github.com/repos/{REPO}/releases?per_page=50")
    } else {
        // 镜像前缀（如 ghproxy）通常代理 api.github.com
        format!("{}/{}", cfg.github_mirror.trim_end_matches('/'), "https://api.github.com/repos/".to_string() + REPO + "/releases?per_page=50")
    };
    let mut cmd = command::hidden("curl");
    cmd.args(["-s", "-H", "User-Agent: dsh-launcher", &api_base]);
    let out = cmd
        .output()
        .map_err(|e| format!("curl 执行失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "查询 GitHub releases 失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        format!("解析 GitHub API 响应失败: {e}（可能是限流或网络问题）")
    })?;
    let arr = json.as_array().ok_or("GitHub API 返回非数组")?;
    let mut versions: Vec<String> = arr
        .iter()
        .filter_map(|v| v.get("tag_name")?.as_str().map(|s| s.to_string()))
        .collect();
    // 按 tag 名降序（最新在前）；alpha 版本自然排序即可
    versions.sort_by(|a, b| b.cmp(a));
    Ok(versions)
}

/// 安装指定版本：clone 源码 + pnpm install + build
/// 返回安装到的目录
pub fn install_version(version: &str) -> Result<String, String> {
    let cfg = AppConfig::load();
    // tag 形如 v0.1.2-alpha.1
    let tag = if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    };
    let dest = github_dsh_dir().join(&tag);

    // 已存在则先清理（全局单版本：覆盖旧版本目录）
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(&dest).map_err(|e| e.to_string())?;

    // clone（可走镜像）
    let clone_url = if cfg.github_mirror.is_empty() {
        format!("https://github.com/{REPO}.git")
    } else {
        format!("{}/{}.git", cfg.github_mirror.trim_end_matches('/'), REPO)
    };

    run_output(
        {
            let mut c = command::hidden("git");
            c.args([
                "clone",
                "--depth",
                "1",
                "--branch",
                &tag,
                &clone_url,
                &dest.to_string_lossy(),
            ]);
            c
        },
        "git clone 失败",
    )?;

    // pnpm install + build（在源码目录内）
    run_output(
        {
            let mut c = command::hidden_cmd("");
            c.arg("install").current_dir(&dest);
            c
        },
        "pnpm install 失败",
    )?;
    run_output(
        {
            let mut c = command::hidden_cmd("");
            c.args(["run", "build"]).current_dir(&dest);
            c
        },
        "pnpm build 失败",
    )?;

    Ok(format!("GitHub 通道 v{version} 构建完成，位于 {}", dest.display()))
}

/// 运行命令并检查退出码
fn run_output(mut cmd: std::process::Command, err_prefix: &str) -> Result<(), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("{err_prefix}: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let msg = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            stdout.trim().to_string()
        };
        return Err(format!("{err_prefix}: {msg}"));
    }
    Ok(())
}
