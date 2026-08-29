//! GitHub 通道核心逻辑：查询 release、clone 源码、pnpm 构建
//!
//! 双通道模型（ADR-0001）：GitHub 通道装 v0.1.2-alpha.1 及更新的源码 tag。
//! 全局单版本（ADR-0003）：当前激活版本 = 最近一次安装的版本。
//! 构建产物放 `%LOCALAPPDATA%\dsh-launcher\github-dsh\<version>\`，dsh bin 由 npm 全局链接。
//!
//! v0.1.7 修复：
//! - `hidden_cmd("")` 空命令 bug（`cmd /D /C ""` 退出码 0 但什么都不执行）
//!   → pnpm install/build 从未真正运行 → 安装"假成功/无响应"；改为 `hidden_cmd("pnpm")`
//! - clone/install/build 改用流式执行（core/stream.rs）：逐行写日志 + 推前端实时流
//!   + 进度事件（core/events.rs）

use crate::core::command;
use crate::core::config::AppConfig;
use crate::core::events::InstallPhase;
use crate::core::logging::{LogLevel, Logger};
use crate::core::stream;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

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
/// 调用方需提供 Logger（用于日志流 + 进度事件）
pub fn install_version(
    version: &str,
    logger: &Arc<Logger>,
) -> Result<String, String> {
    let cfg = AppConfig::load();
    // 注意：GitHub release tag 命名形如 `dsh-v0.1.2-alpha.1`（带 dsh- 前缀）。
    // list_releases 返回的 tag_name 即为完整 tag，这里必须原样使用，
    // 不能自作主张加 v 前缀（否则会变成 vdsh-v0.1.2-alpha.1 → branch not found）。
    let tag = version.trim().to_string();
    if tag.is_empty() {
        return Err("版本号为空".to_string());
    }
    let dest = github_dsh_dir().join(&tag);

    // 已存在则先清理（全局单版本：覆盖旧版本目录）
    logger.info(&format!("清理旧目录 {}", dest.display()));
    logger.progress("github", InstallPhase::Prepare, 0, "准备安装目录…");
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

    logger.info(&format!("开始克隆 {clone_url} (tag={tag})…"));
    logger.progress("github", InstallPhase::Download, 0, "git clone 开始…");
    let cb: Arc<stream::LineCallback> = {
        let logger = Arc::clone(logger);
        Arc::new(move |_lvl, line| {
            // 解析 git clone 的接收进度：Receiving objects:  45%
            if let Some(p) = parse_git_percent(line) {
                logger.progress("github", InstallPhase::Download, p, "正在克隆源码…");
            }
        })
    };
    {
        let mut c = command::hidden("git");
        c.args([
            "clone",
            "--depth",
            "1",
            "--branch",
            &tag,
            "--progress",
            &clone_url,
            &dest.to_string_lossy(),
        ]);
        stream::run_streamed(logger, c, LogLevel::Info, LogLevel::Warn, Some(cb))
            .map_err(|e| format!("git clone 失败: {e}"))?;
    }
    logger.progress("github", InstallPhase::Download, 100, "源码克隆完成");

    // pnpm install + build（在源码目录内）
    // v0.1.7 修复：此前 hidden_cmd("") 是空命令（cmd /D /C "" 退出码 0 但什么都不做），
    // pnpm 从未执行 → 安装假成功。改为 hidden_cmd("pnpm")。
    logger.progress("github", InstallPhase::Install, 0, "pnpm install 开始…");
    logger.info("开始 pnpm install（安装依赖）…");
    stream::run_cmd_script(logger, "pnpm", &["install".to_string()], Some(&dest), LogLevel::Info, None)
        .map_err(|e| format!("pnpm install 失败: {e}"))?;
    logger.progress("github", InstallPhase::Install, 100, "依赖安装完成");

    logger.progress("github", InstallPhase::Build, 0, "pnpm build 开始…");
    logger.info("开始 pnpm build（构建产物）…");
    stream::run_cmd_script(
        logger,
        "pnpm",
        &["run".to_string(), "build".to_string()],
        Some(&dest),
        LogLevel::Info,
        None,
    )
    .map_err(|e| format!("pnpm build 失败: {e}"))?;
    logger.progress("github", InstallPhase::Build, 100, "构建完成");
    logger.progress("github", InstallPhase::Done, 100, "安装完成");

    Ok(format!("GitHub 通道 {version} 构建完成，位于 {}", dest.display()))
}

/// 从 git clone 输出行解析接收百分比（Receiving objects: 45%）
fn parse_git_percent(line: &str) -> Option<u8> {
    let p = line.find('%')?;
    // 往前找数字段
    let before = &line[..p];
    let digits_start = before
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| i + 1)
        .unwrap_or(0);
    let num: u32 = before[digits_start..].trim().parse().ok()?;
    Some(num.min(100) as u8)
}

#[cfg(test)]
mod tests {
    use super::parse_git_percent;

    #[test]
    fn test_parse_git_percent() {
        // 标准 git clone 进度行（含大小/速率）
        assert_eq!(parse_git_percent("Receiving objects:  45% (45/100)"), Some(45));
        assert_eq!(parse_git_percent("Receiving objects: 100% (100/100)"), Some(100));
        assert_eq!(parse_git_percent("Receiving objects: 12%, 3.4 MiB | 1.2 MiB/s"), Some(12));
        // 非进度行不解析
        assert_eq!(parse_git_percent("Cloning into 'foo'..."), None);
        assert_eq!(parse_git_percent("remote: Enumerating objects: 5"), None);
        // 超 100 封顶
        assert_eq!(parse_git_percent("Receiving objects: 105%"), Some(100));
    }
}
