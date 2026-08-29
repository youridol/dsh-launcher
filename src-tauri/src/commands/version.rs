//! 版本与通道管理 IPC
//!
//! 双通道模型（ADR-0001）：
//! - npm 通道：registry 现有版本（当前最新 0.1.1-rc.2）
//! - GitHub 通道：v0.1.2-alpha.1 及更新源码 tag（core/github.rs）
//! 全局单版本（ADR-0003）：切换 = 先卸载再装
//!
//! 并发策略：所有命令 async + spawn_blocking（npm/git 是网络/磁盘 IO，不占 IPC 线程）
//! v0.1.7：安装改为流式执行（core/stream.rs）+ 进度事件（core/events.rs），
//! 输出实时写日志并推前端，避免"无响应"。

use crate::core::command;
use crate::core::github as core_github;
use crate::core::logging::LogLevel;
use crate::core::stream;
use crate::AppState;
use serde::Serialize;
use std::sync::Arc;
use tauri::State;

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

/// 获取某通道的可用版本列表（异步）
/// v0.1.9：读取失败/空列表时写入日志（落盘 + 前端实时流）
#[tauri::command]
pub async fn list_versions(
    state: State<'_, AppState>,
    channel: String,
) -> Result<Vec<DshVersion>, String> {
    let logger = Arc::clone(&state.logger);
    tauri::async_runtime::spawn_blocking(move || {
        let result = match channel.as_str() {
            "npm" => list_npm_versions(),
            "github" => list_github_versions(),
            other => Err(format!("未知通道: {other}")),
        };
        // 读取不到版本时写入日志（便于在日志面板定位网络/镜像问题）
        match &result {
            Ok(versions) if versions.is_empty() => {
                logger.warn(&format!(
                    "读取 {channel} 通道版本列表成功但为空（请检查网络/镜像源）"
                ));
            }
            Err(e) => {
                logger.error(&format!("读取 {channel} 通道版本列表失败: {e}"));
            }
            _ => {}
        }
        result
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 已安装的 dsh 版本（全局单版本；dsh --version 是进程调用，放后台）
#[tauri::command]
pub async fn get_installed_version() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut c = command::hidden("dsh");
        c.arg("--version");
        let out = c.output().map_err(|e| format!("dsh 未安装或不可用: {e}"))?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(String::from_utf8(out.stdout)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()))
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 安装指定版本（npm 通道：npm i -g；GitHub 通道：clone + build）
/// v0.1.7：流式执行 + 进度事件 + 日志实时输出
#[tauri::command]
pub async fn install_version(
    state: State<'_, AppState>,
    channel: String,
    version: String,
) -> Result<String, String> {
    let logger = Arc::clone(&state.logger);
    tauri::async_runtime::spawn_blocking(move || match channel.as_str() {
        "npm" => install_npm_version(&logger, &version),
        "github" => core_github::install_version(&version, &logger),
        other => Err(format!("未知通道: {other}")),
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 卸载 dsh（npm 全局 + GitHub 通道目录）
#[tauri::command]
pub async fn uninstall() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        // 1. 卸载 npm 全局包
        let _ = {
            let mut c = command::hidden_cmd("npm");
            c.args(["uninstall", "-g", "@deepseek-ai/dsh"]);
            c.output()
        };
        // 2. 清理 GitHub 通道目录
        let gh_dir = core_github::github_dsh_dir();
        if gh_dir.exists() {
            std::fs::remove_dir_all(&gh_dir).map_err(|e| e.to_string())?;
        }
        Ok::<_, String>("dsh 已卸载（npm 全局包 + GitHub 源码目录）".to_string())
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

fn list_npm_versions() -> Result<Vec<DshVersion>, String> {
    let mut c = command::hidden_cmd("npm");
    c.args(["view", "@deepseek-ai/dsh", "versions", "--json"]);
    let out = c
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

/// npm 通道安装：流式执行 npm install -g，输出写日志 + 进度事件
fn install_npm_version(logger: &Arc<crate::core::logging::Logger>, version: &str) -> Result<String, String> {
    let spec = format!("@deepseek-ai/dsh@{version}");
    logger.info(&format!("开始 npm 安装 {spec} …"));
    // npm 在非 TTY 下无逐字节进度，采用阶段估算：
    // 每收到一行输出步进 5%，到 90% 封顶，完成时置 100。
    // Fn 闭包需 Send+Sync，用 Arc<AtomicU8> 保存步进状态
    let step = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let cb: Arc<stream::LineCallback> = {
        let logger = Arc::clone(logger);
        let step = Arc::clone(&step);
        Arc::new(move |_lvl, line| {
            if line.starts_with("npm warn") || line.starts_with("npm error") || line.starts_with("npm ERR") {
                return;
            }
            use std::sync::atomic::Ordering;
            let s = (step.load(Ordering::Relaxed) + 2).min(90);
            step.store(s, Ordering::Relaxed);
            logger.progress("npm", crate::core::events::InstallPhase::Install, s, line.to_string());
        })
    };
    let mut c = command::hidden_cmd("npm");
    c.args(["install", "-g", &spec]);
    stream::run_streamed(logger, c, LogLevel::Info, LogLevel::Warn, Some(cb))
        .map_err(|e| format!("npm 安装失败: {e}"))?;
    logger.progress(
        "npm",
        crate::core::events::InstallPhase::Done,
        100,
        "安装完成",
    );
    Ok(format!("已安装 dsh {version}（npm 通道）"))
}
