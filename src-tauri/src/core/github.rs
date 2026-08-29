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
    // -f：HTTP 4xx/5xx 时 curl 返回非零退出码（否则 -s 静默吞掉错误响应）
    // -L：跟随 307/308 重定向（镜像可能跳转）
    // -w：把 HTTP 状态码追加到 stdout，便于诊断
    cmd.args([
        "-sS",
        "-fL",
        "-w",
        "\\n__HTTP_CODE__:%{http_code}",
        "-H",
        "User-Agent: dsh-launcher",
        &api_base,
    ]);
    let out = cmd
        .output()
        .map_err(|e| format!("curl 执行失败: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // -f 失败时 curl 会把响应体输出到 stdout（限流/错误信息），尝试提取 message
        let text = String::from_utf8_lossy(&out.stdout);
        let hint = extract_api_error_message(&text).unwrap_or_else(|| {
            if stderr.is_empty() {
                format!("HTTP 请求失败（退出码 {})", out.status.code().unwrap_or(-1))
            } else {
                stderr
            }
        });
        return Err(format!("查询 GitHub releases 失败: {hint}"));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // 去掉 -w 追加的状态码标记行
    let text = text
        .rsplit_once("__HTTP_CODE__:")
        .map(|(body, _)| body)
        .unwrap_or(&text);
    let json: serde_json::Value = serde_json::from_str(text.trim()).map_err(|e| {
        format!(
            "解析 GitHub API 响应失败: {e}（响应可能不是 JSON，请检查镜像源/网络）"
        )
    })?;
    // GitHub API 错误返回 JSON 对象（如限流 message）；数组才是正常 releases
    let arr = match json.as_array() {
        Some(arr) => arr,
        None => {
            // 尝试提取 API 返回的错误信息，给出可操作的提示
            let hint = extract_api_error_message(&text).unwrap_or_else(|| {
                "响应不是版本列表数组（请检查 GitHub 网络/镜像源/是否限流）".to_string()
            });
            return Err(format!("查询 GitHub releases 失败: {hint}"));
        }
    };
    let mut versions: Vec<String> = arr
        .iter()
        .filter_map(|v| v.get("tag_name")?.as_str().map(|s| s.to_string()))
        .collect();
    // 按 tag 名降序（最新在前）；alpha 版本自然排序即可
    versions.sort_by(|a, b| b.cmp(a));
    Ok(versions)
}

/// 从 GitHub API 错误响应中提取 message 字段（限流/仓库不存在等）
fn extract_api_error_message(text: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let msg = json.get("message")?.as_str()?.to_string();
    // 附带限流提示，便于用户理解
    let hint = if msg.to_lowercase().contains("rate limit") {
        format!("{msg}（GitHub API 未认证限流 60 次/小时，稍后再试或配置镜像源）")
    } else {
        msg
    };
    Some(hint)
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
    use super::extract_api_error_message;

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
    fn test_extract_api_error_message() {
        // GitHub API 限流响应（对象 + message）
        let rate = r#"{"message":"API rate limit exceeded for 1.2.3.4.","documentation_url":"https://docs.github.com/rest"}"#;
        let hint = extract_api_error_message(rate).unwrap();
        assert!(hint.contains("rate limit"), "应含限流提示: {hint}");
        assert!(hint.contains("限流"), "应含中文限流说明: {hint}");

        // 仓库不存在响应
        let not_found = r#"{"message":"Not Found"}"#;
        assert_eq!(extract_api_error_message(not_found).as_deref(), Some("Not Found"));

        // 非 JSON（如镜像返回 HTML）→ None，由调用方给出通用提示
        assert_eq!(extract_api_error_message("<html>502 Bad Gateway</html>"), None);
        assert_eq!(extract_api_error_message(""), None);

        // 正常数组响应 → None（不需要错误信息）
        assert_eq!(extract_api_error_message(r#"[{"tag_name":"dsh-v0.1.2-alpha.1"}]"#), None);
    }
}
