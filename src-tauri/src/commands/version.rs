//! 版本与通道管理 IPC
//!
//! 双通道模型（ADR-0001）：
//! - npm 通道：registry 现有版本（当前最新 0.1.1-rc.2）
//! - GitHub 通道：v0.1.2-alpha.1 及更新源码 tag（core/github.rs）
//!
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

/// 已安装的 dsh 版本（全局单版本）
/// v0.2.3：优先检测 GitHub 安装目录（github-dsh\deepseek-harness），
/// 再尝试 PATH 中的 dsh --version（npm 全局）。修复 GitHub 安装后状态不更新。
///
/// v0.4.6：执行 dsh --version 前先 probe_dsh_command 静态解析——本启动器 shim 指向
/// 的安装目录缺失/被清空时，执行 dsh --version 会触发 pnpm 递归进程爆炸
/// （cmd → node(pnpm) → cmd → ... 指数增长，见 github.rs::DshProbe），必须短路返回 None。
#[tauri::command]
pub async fn get_installed_version() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        // 1. GitHub 安装目录存在 → 返回目录中 package.json 版本（或标记 github）
        if crate::core::github::github_installed() {
            let dir = crate::core::github::github_clone_dir();
            let pkg = dir.join("package.json");
            if let Ok(content) = std::fs::read_to_string(&pkg) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(v) = json.get("version").and_then(|v| v.as_str()) {
                        return Ok(Some(format!("github:{v}")));
                    }
                }
            }
            return Ok(Some("github:已安装".to_string()));
        }
        // 2. 静态解析 PATH 中 dsh.cmd：本启动器 shim 指向损坏目录 → 短路（避免递归爆炸）
        match crate::core::github::probe_dsh_command() {
            crate::core::github::DshProbe::OwnedShimBroken => {
                // 安装目录已被卸载/清空：不执行 dsh（执行会 pnpm 递归爆炸），视为未安装
                return Ok(None);
            }
            crate::core::github::DshProbe::None => {
                return Ok(None);
            }
            // OwnedShimOk（目标目录有效，虽不应在此分支，但执行是安全的）与 Foreign 继续
            crate::core::github::DshProbe::OwnedShimOk
            | crate::core::github::DshProbe::Foreign => {}
        }
        // 3. PATH 中的 dsh --version（npm 全局）。Windows 上 dsh 是 .cmd shim，
        //    不能直接 spawn（program not found），必须 cmd.exe /C 包装
        let mut c = command::hidden_cmd("dsh");
        c.arg("--version");
        let out = c.output().map_err(|e| format!("dsh 未安装或不可用: {e}"))?;
        if !out.status.success() {
            return Ok(None);
        }
        let ver = crate::core::text::decode(&out.stdout);
        let ver = ver.trim().to_string();
        Ok(if ver.is_empty() {
            None
        } else {
            Some(format!("npm:{ver}"))
        })
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 安装指定版本（npm 通道：npm i -g；GitHub 通道：clone + build）
/// v0.1.7：流式执行 + 进度事件 + 日志实时输出；成功后广播 version://changed
///
/// v0.4.9：**安装前自动卸载对侧通道版本**（ADR-0003 全局单版本的彻底化）。
/// 此前两个通道的 dsh 各自独立安装：npm 全局包残留 node_modules、GitHub 源码目录
/// 残留数 GB，且 dsh.cmd 同名互相覆盖，切换不干净（旧版残留影响启动决策）。
/// 现在：
/// - 装 npm 通道 → 先清理 GitHub 通道（删除源码目录 + 自家 shim；不清 DSH_HOME 数据）
/// - 装 GitHub 通道 → 先卸载 npm 全局包（npm uninstall -g @deepseek-ai/dsh）
/// 保证任何时刻全局只有一个通道的 dsh 生效。
#[tauri::command]
pub async fn install_version(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    channel: String,
    version: String,
) -> Result<String, String> {
    let logger = Arc::clone(&state.logger);
    let process = std::sync::Arc::clone(&state.process);
    tauri::async_runtime::spawn_blocking(move || {
        // 切换通道前统一优雅停止 dsh（ADR-0002：卸载/替换前必须停止，避免运行中
        // 占用安装目录/全局包导致删除失败；装完由用户重新启动）
        logger.info("切换通道安装前尝试停止 dsh（若有运行）…");
        if let Err(e) = process.stop() {
            logger.warn(&format!("停止 dsh 失败（继续安装）: {e}"));
        }
        let result = match channel.as_str() {
            "npm" => {
                // 先卸载对侧 GitHub 通道（若有），再装 npm
                if let Err(e) = uninstall_github_channel(&logger) {
                    logger.warn(&format!("清理 GitHub 通道失败（继续 npm 安装）: {e}"));
                }
                install_npm_version(&logger, &version)
            }
            "github" => {
                // 先卸载对侧 npm 全局包（若有），再装 GitHub
                if let Err(e) = uninstall_npm_global(&logger) {
                    logger.warn(&format!("卸载 npm 全局包失败（继续 GitHub 安装）: {e}"));
                }
                core_github::install_version(&version, &logger)
            }
            other => Err(format!("未知通道: {other}")),
        };
        if result.is_ok() {
            // 安装成功 → 广播版本变更（前端刷新已安装版本/通道状态）
            crate::core::events::emit_version_changed(&app);
        }
        result
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 卸载 npm 全局 dsh 包（幂等：未安装时静默成功）
fn uninstall_npm_global(logger: &Arc<crate::core::logging::Logger>) -> Result<(), String> {
    logger.info("卸载 npm 全局包 @deepseek-ai/dsh…");
    let npm_result = {
        let mut c = command::hidden_cmd("npm");
        c.args(["uninstall", "-g", "@deepseek-ai/dsh"]);
        c.output()
    };
    match &npm_result {
        Ok(out) if out.status.success() => {
            logger.info("npm 全局包已卸载");
            Ok(())
        }
        Ok(out) => {
            let msg = format!(
                "npm uninstall 返回非零: {}",
                crate::core::text::decode(&out.stderr).trim()
            );
            logger.warn(&msg);
            Ok(()) // npm 包不存在时返回码也非零，视为已干净
        }
        Err(e) => {
            logger.warn(&format!("npm uninstall 执行失败（可能未通过 npm 安装）: {e}"));
            Ok(()) // 非 npm 通道安装（无全局包）→ 无操作成功
        }
    }
}

/// 卸载 GitHub 通道（删除源码目录 + 自家全局 shim；**不清 DSH_HOME 用户数据**）
/// 幂等：目录/shim 不存在时静默成功。返回 Err 仅当目录删除失败（被占用）。
/// 注意：调用方应确保 dsh 已停止（本函数不负责停 dsh——卸载命令/切换安装统一先停）。
fn uninstall_github_channel(
    logger: &Arc<crate::core::logging::Logger>,
) -> Result<(), String> {
    // 1. 删除 GitHub 源码目录（github-dsh\deepseek-harness）
    let gh_dir = core_github::github_clone_dir();
    if gh_dir.exists() {
        logger.info(&format!("清理 GitHub 源码目录: {}", gh_dir.display()));
        std::fs::remove_dir_all(&gh_dir)
            .map_err(|e| format!("清理 GitHub 源码目录失败: {e}"))?;
    } else {
        logger.info("GitHub 源码目录不存在，跳过清理");
    }
    // 2. 清理全局 dsh.cmd shim（GitHub 安装时创建的；npm 全局包安装时 npm 会写自己的）
    let shim = core_github::global_shim_path();
    if let Some(p) = shim {
        if p.exists() {
            match std::fs::remove_file(&p) {
                Ok(_) => logger.info(&format!("已删除全局 dsh 命令: {}", p.display())),
                Err(e) => logger.warn(&format!("删除 dsh.cmd shim 失败（继续）: {e}")),
            }
        }
    }
    Ok(())
}

/// 卸载 dsh（npm 全局 + GitHub 通道目录 + 可选 DSH_HOME 用户数据）
/// 完成后广播 version://changed（前端版本管理/状态卡联动刷新）
#[tauri::command]
pub async fn uninstall(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let logger = Arc::clone(&state.logger);
    let process = std::sync::Arc::clone(&state.process);
    tauri::async_runtime::spawn_blocking(move || {
        // 卸载前先优雅停止 dsh（ADR-0002：卸载前必须停止，避免删除运行中目录失败）
        logger.info("卸载前优雅停止 dsh…");
        if let Err(e) = process.stop() {
            logger.warn(&format!("停止 dsh 失败（继续卸载）: {e}"));
        }
        logger.info("开始卸载 dsh（npm 全局包 + GitHub 源码目录 + 全局命令）…");
        // 1. 卸载 npm 全局包（幂等）
        uninstall_npm_global(&logger)?;
        // 2. 清理 GitHub 通道（删源码目录 + 自家 shim；幂等）
        uninstall_github_channel(&logger)?;
        // 3. DSH_HOME 用户数据（卸载开关 keepDshHomeOnUninstall）：
        //    默认保留（不接管、不碰用户数据，见 ADR-0002）；仅当用户显式关闭保留开关时
        //    才删除 dsh 官方默认数据目录 ~/.dsh，且先确认目录特征（避免误删非 dsh 数据）。
        let cfg = crate::core::config::AppConfig::load();
        if !cfg.keep_dsh_home_on_uninstall {
            let dsh_home = std::env::var("USERPROFILE")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join(".dsh");
            if dsh_home.is_dir() {
                // 特征确认：dsh 数据目录含 profiles 子目录或 settings.yaml（避免误删）
                let looks_like_dsh = dsh_home.join("profiles").is_dir()
                    || dsh_home.join("settings.yaml").exists();
                if looks_like_dsh {
                    match std::fs::remove_dir_all(&dsh_home) {
                        Ok(_) => logger.info(&format!(
                            "已删除 dsh 用户数据目录（keepDshHomeOnUninstall=false）: {}",
                            dsh_home.display()
                        )),
                        Err(e) => {
                            logger.warn(&format!("删除 DSH_HOME 失败（请手动清理）: {e}"));
                        }
                    }
                } else {
                    logger.warn(&format!(
                        "{} 存在但无 dsh 数据特征，为安全起见不删除（请确认后手动清理）",
                        dsh_home.display()
                    ));
                }
            }
        } else {
            logger.info("卸载保留 DSH_HOME（keepDshHomeOnUninstall=true，不碰用户数据）");
        }
        // 广播版本变更（前端刷新安装状态）
        crate::core::events::emit_version_changed(&app);
        logger.info("dsh 已卸载完成");
        Ok::<_, String>("dsh 已卸载（npm 全局包 + GitHub 源码目录 + 全局命令）".to_string())
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
            crate::core::text::decode(&out.stderr)
        ));
    }
    let text = crate::core::text::decode(&out.stdout);
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

/// 安装路径信息（任务：显示 harness 下载/安装目录）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPaths {
    /// GitHub 通道克隆/构建目录
    pub github_dir: String,
    /// GitHub 已安装
    pub github_installed: bool,
    /// npm 全局包目录（npm prefix -g）
    pub npm_global_dir: String,
    /// npm 全局 bin 目录
    pub npm_bin_dir: String,
}

/// 获取 harness 下载/安装目录（本地文件系统，无网络）
#[tauri::command]
pub async fn get_install_paths() -> InstallPaths {
    tauri::async_runtime::spawn_blocking(|| {
        let github_dir = crate::core::github::github_clone_dir();
        let npm_prefix = core_github::npm_prefix_dir();
        InstallPaths {
            github_dir: github_dir.to_string_lossy().to_string(),
            github_installed: crate::core::github::github_installed(),
            npm_global_dir: npm_prefix.to_string_lossy().to_string(),
            // Windows：npm 全局命令目录 = prefix 本身（PATH 条目即 prefix）
            npm_bin_dir: npm_prefix.to_string_lossy().to_string(),
        }
    })
    .await
    .unwrap_or(InstallPaths {
        github_dir: String::new(),
        github_installed: false,
        npm_global_dir: String::new(),
        npm_bin_dir: String::new(),
    })
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
