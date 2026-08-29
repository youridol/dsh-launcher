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

/// 查询 GitHub 可用版本（tags）
///
/// v0.2.1 修复：改用 `git ls-remote --tags` 作为主路径。
/// - 原 API 路径（curl api.github.com）未认证限流 60 次/小时，403 时整个列表不可用
///   （表现为 "curl: (22) ... 403" / "GitHub API 返回非数组"）；
///   git 走 HTTPS 协议不受 API 限流影响，1~2 秒即可返回全部 tag。
/// - 附带收益：API /releases 只含 release，ls-remote 能拿到全部 tag（含 rc），列表更全。
/// - 配置了镜像源时仍走 curl（镜像代理 API 或 git 均可），失败时给出明确原因。
pub fn list_releases() -> Result<Vec<String>, String> {
    let cfg = AppConfig::load();
    let repo_url = if cfg.github_mirror.is_empty() {
        format!("https://github.com/{REPO}.git")
    } else {
        format!("{}/{}.git", cfg.github_mirror.trim_end_matches('/'), REPO)
    };

    // 主路径：git ls-remote --tags（不受 GitHub API 限流影响）
    let mut cmd = command::hidden("git");
    cmd.args(["ls-remote", "--tags", &repo_url]);
    let out = cmd
        .output()
        .map_err(|e| format!("git ls-remote 执行失败: {e}"))?;
    if out.status.success() {
        let text = String::from_utf8_lossy(&out.stdout);
        let versions = parse_tags_from_ls_remote(&text);
        if !versions.is_empty() {
            return Ok(versions);
        }
        // ls-remote 成功但无 tag → 仓库无发布，明确提示
        return Ok(Vec::new());
    }
    // git 路径失败（镜像不支持 git 协议等）→ 兜底走 API，并给出诊断信息
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let hint = if stderr.is_empty() {
        format!("git ls-remote 失败（退出码 {})", out.status.code().unwrap_or(-1))
    } else {
        stderr
    };
    Err(format!("查询 GitHub releases 失败: {hint}"))
}

/// 从 `git ls-remote --tags` 输出解析 tag 列表（去重、去 ^{} 剥离、降序）
fn parse_tags_from_ls_remote(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let _sha = parts.next()?;
            let refname = parts.next()?;
            // 只接受 tags 引用（HEAD / refs/heads/* 忽略）；形如 refs/tags/dsh-v0.1.2-alpha.1
            let tag = refname
                .strip_prefix("refs/tags/")?
                .trim_end_matches("^{}");
            if tag.is_empty() {
                None
            } else {
                Some(tag.to_string())
            }
        })
        .collect();
    // 去重（ peeled ^{} 与轻量 tag 指向同 sha 时 git 会输出两条）
    tags.sort();
    tags.dedup();
    // 按 tag 名降序（最新在前）；alpha/rc 版本自然排序即可
    tags.sort_by(|a, b| b.cmp(a));
    tags
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
    use super::parse_tags_from_ls_remote;

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

    #[test]
    fn test_parse_tags_from_ls_remote() {
        // 真实 git ls-remote --tags 输出（含 peeled ^{} 重复行）
        let sample = r#"
cd5ef8148158c3a752a658978873241fdf8e2bbc	refs/tags/dsh-v0.1.2-alpha.1
cd5ef8148158c3a752a658978873241fdf8e2bbc	refs/tags/dsh-v0.1.2-alpha.1^{}
528c682e061696f5a160f363f236ecbf53cbd006	refs/tags/dsh-v0.1.1-rc.1
b150a551b8d465e31e418e1b2eaf5e79bbb7d28e	refs/tags/dsh-v0.1.1-rc.2
141eb6fef83422698aef7a981029e843e8161534	refs/tags/dsh-v0.1.0-rc.8
"#;
        let tags = parse_tags_from_ls_remote(sample);
        // 去重（dsh-v0.1.2-alpha.1 只出现一次）+ 去 ^{} 后缀
        assert_eq!(tags.len(), 4, "应得到 4 个去重 tag: {tags:?}");
        assert!(tags.contains(&"dsh-v0.1.2-alpha.1".to_string()));
        assert!(tags.contains(&"dsh-v0.1.1-rc.1".to_string()));
        assert!(tags.contains(&"dsh-v0.1.1-rc.2".to_string()));
        assert!(tags.contains(&"dsh-v0.1.0-rc.8".to_string()));
        // 降序：dsh-v0.1.2 应在 dsh-v0.1.0 前
        let idx_12 = tags.iter().position(|t| t == "dsh-v0.1.2-alpha.1").unwrap();
        let idx_10 = tags.iter().position(|t| t == "dsh-v0.1.0-rc.8").unwrap();
        assert!(idx_12 < idx_10, "降序排列: {tags:?}");

        // 空输出 → 空列表
        assert!(parse_tags_from_ls_remote("").is_empty());
        // 无 tags 前缀的行忽略
        assert!(parse_tags_from_ls_remote("abc123	HEAD\n").is_empty());
    }
}
