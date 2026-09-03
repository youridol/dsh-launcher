//! 工具链检测、安装、卸载、批量操作 IPC
//!
//! 清单（CONTEXT.md）：Node.js(22.19+/24+)、npm、pnpm、Git(2.26+)、Python(可选)
//! 状态：present / missing / mismatch
//!
//! 安装策略：
//! - Node：官方 zip 解压到用户级目录（免管理员）+ 写用户 PATH（见 core/toolchain.rs / pathutil.rs）
//! - Git：官方安装包，需提权 → runas（Q22）
//! - pnpm：npm i -g（随 node 用户级目录；npm 找不到时 PATH 注入 node_dir）
//! - Python：官方完整安装包静默安装（提权，/PrependPath）
//!
//! 卸载策略：
//! - Node：删用户级目录 + 清用户 PATH 条目（core/toolchain::uninstall_node）
//! - Git/Python：注册表 UninstallString → 官方卸载器提权（core/toolchain::uninstall_git/python）
//!   （不影响非官方通道安装的 Git/Python —— 找不到卸载入口时明确报错提示）
//!
//! 并发策略：detect/install/uninstall 都是多进程调用 + 网络下载，全部 async + spawn_blocking，
//! 互不阻塞（各占独立后台线程）。
//!
//! 编码：所有子进程输出统一 core::text::decode（UTF-8 优先 + 代码页回退），
//! 修复 Windows 中文系统 cmd/npm 报错乱码（"不是内部或外部命令" 等）。

use crate::core::command;
use crate::core::events::InstallPhase;
use crate::core::logging::Logger;
use crate::core::toolchain as core_toolchain;
use serde::Serialize;
use std::sync::Arc;

/// 工具链项
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainItem {
    pub name: &'static str,
    pub installed_version: Option<String>,
    pub required: &'static str,
    pub state: &'static str,
}

/// 批量操作结果（一条工具链一个子结果）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchResult {
    pub name: String,
    pub ok: bool,
    pub message: String,
}

/// 检测所有工具链（异步：多进程版本探测放后台）
#[tauri::command]
pub async fn detect_toolchain() -> Vec<ToolchainItem> {
    tauri::async_runtime::spawn_blocking(|| {
        vec![
            detect_node(),
            detect_npm(),
            detect_pnpm(),
            detect_git(),
            detect_python(),
        ]
    })
    .await
    .unwrap_or_default()
}

/// 安装指定工具链（异步：下载/安装放后台）
/// v0.4.0：支持 python；卸载单独走 uninstall_toolchain
/// 完成后广播 toolchain://changed（前端实时刷新条目状态）
#[tauri::command]
pub async fn install_toolchain(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    name: String,
) -> Result<String, String> {
    let logger = Arc::clone(&state.logger);
    tauri::async_runtime::spawn_blocking(move || {
        let result = match name.as_str() {
            "node" => install_node(&logger),
            "git" => install_git(&logger),
            "pnpm" => install_pnpm(&logger),
            "python" => install_python(&logger),
            other => Err(format!("不支持的工具链: {other}")),
        };
        // 广播工具链变更（前端刷新条目状态 present/missing）。
        // 例外：git/python 为 UAC 异步安装（Start-Process 立即返回或异步执行），
        // 此时广播会误报“仍缺失”；由 ToolchainPanel 显示 UAC 提示并等用户完成后再手动刷新
        if result.is_ok() && matches!(name.as_str(), "node" | "pnpm") {
            crate::core::events::emit_toolchain_changed(&app);
        }
        result
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 卸载指定工具链（异步）
/// Node 为同步（删目录 + PATH）；Git/Python 为 UAC 异步卸载器
#[tauri::command]
pub async fn uninstall_toolchain(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    name: String,
) -> Result<String, String> {
    let logger = Arc::clone(&state.logger);
    let process = std::sync::Arc::clone(&state.process);
    tauri::async_runtime::spawn_blocking(move || {
        // 卸载 Node 前先优雅停止 dsh（dsh 进程可能正用 node_dir 的 node.exe/pnpm，
        // 占用会导致目录删除失败）
        if name == "node" {
            logger.info("卸载 Node 前尝试停止 dsh（若有占用）…");
            if let Err(e) = process.stop() {
                logger.warn(&format!("停止 dsh 失败（继续卸载）: {e}"));
            }
        }
        let result = match name.as_str() {
            "node" => core_toolchain::uninstall_node(&logger),
            "git" => core_toolchain::uninstall_git(&logger),
            "python" => core_toolchain::uninstall_python(&logger),
            // npm/pnpm 随 Node 目录删除（无独立卸载：npm/pnpm 是本启动器安装 Node 时
            // 随 node zip 带入 + npm i -g pnpm 装进同一 node 目录，卸载 Node 即清理）
            "npm" | "pnpm" => Err(
                "npm/pnpm 随 Node 安装目录管理，卸载 Node 时一并清理（无需单独卸载）".to_string(),
            ),
            other => Err(format!("不支持的工具链: {other}")),
        };
        if result.is_ok() && name == "node" {
            crate::core::events::emit_toolchain_changed(&app);
        }
        result
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 批量一键安装缺失工具链：一次性安装 node + pnpm（并触发 git/python 的 UAC 安装器）
/// 按依赖顺序：node → pnpm（依赖 node）。返回每个工具链的结果（含跳过项）。
#[tauri::command]
pub async fn batch_install_toolchains(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<BatchResult>, String> {
    let logger = Arc::clone(&state.logger);
    tauri::async_runtime::spawn_blocking(move || {
        let mut results = Vec::new();
        // 安装前先检测（跳过已就绪的）
        let detected = detect_all();
        let by_name: std::collections::HashMap<&str, ToolchainItem> = detected
            .iter()
            .map(|d| (d.name, d.clone()))
            .collect();
        let missing: Vec<&str> = ["node", "pnpm", "git", "python"]
            .into_iter()
            .filter(|n| {
                by_name
                    .get(n)
                    .map(|d| d.state != "present")
                    .unwrap_or(true)
            })
            .collect();
        logger.info(&format!("批量安装开始，待安装: {:?}", missing));

        // node 必须先于 pnpm（pnpm 经 npm i -g 装到 node 目录）
        for name in ["node", "pnpm", "git", "python"] {
            if !missing.contains(&name) {
                results.push(BatchResult {
                    name: name.to_string(),
                    ok: true,
                    message: "已就绪，跳过".to_string(),
                });
                continue;
            }
            let r = match name {
                "node" => install_node(&logger),
                "pnpm" => install_pnpm(&logger),
                "git" => install_git(&logger),
                "python" => install_python(&logger),
                _ => unreachable!(),
            };
            match r {
                Ok(msg) => results.push(BatchResult {
                    name: name.to_string(),
                    ok: true,
                    message: msg,
                }),
                Err(e) => results.push(BatchResult {
                    name: name.to_string(),
                    ok: false,
                    message: e,
                }),
            }
        }
        // 广播（git/python UAC 异步不广播，前端显示提示）
        crate::core::events::emit_toolchain_changed(&app);
        Ok(results)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 一键卸载全部（除 npm/pnpm——随 Node 目录一并删除）。
/// 顺序：先同步卸载 node（若装了用户级 Node），再逐个触发 git/python 卸载器。
/// npm/pnpm 若由其他安装器装到别处，此处不强行卸载（避免误删用户系统环境）。
#[tauri::command]
pub async fn batch_uninstall_toolchains(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<BatchResult>, String> {
    let logger = Arc::clone(&state.logger);
    let process = std::sync::Arc::clone(&state.process);
    tauri::async_runtime::spawn_blocking(move || {
        // 批量卸载前先停止 dsh（node_dir 被 dsh 占用会导致删除失败）
        logger.info("批量卸载前尝试停止 dsh（若有占用）…");
        if let Err(e) = process.stop() {
            logger.warn(&format!("停止 dsh 失败（继续卸载）: {e}"));
        }
        let mut results = Vec::new();
        for name in ["node", "git", "python"] {
            let r = match name {
                "node" => core_toolchain::uninstall_node(&logger),
                "git" => core_toolchain::uninstall_git(&logger),
                "python" => core_toolchain::uninstall_python(&logger),
                _ => unreachable!(),
            };
            match r {
                Ok(msg) => results.push(BatchResult {
                    name: name.to_string(),
                    ok: true,
                    message: msg,
                }),
                Err(e) => results.push(BatchResult {
                    name: name.to_string(),
                    ok: false,
                    message: e,
                }),
            }
        }
        crate::core::events::emit_toolchain_changed(&app);
        Ok(results)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 检测全部（同步版；供 batch 逻辑复用）
fn detect_all() -> Vec<ToolchainItem> {
    vec![
        detect_node(),
        detect_npm(),
        detect_pnpm(),
        detect_git(),
        detect_python(),
    ]
}

/// 运行命令取 stdout（首行）
/// npm/pnpm 是 .cmd（batch），需用 cmd.exe 包装；其余 .exe 直接执行。
/// 先查系统 PATH；PATH 无则回退用户级 Node 目录（npm/pnpm 随 node zip 安装，
/// 见 core/toolchain.rs node_dir）——保证“已安装 Node 但 PATH 未注入”时状态仍一致。
fn run_cmd(bin: &str, args: &[&str]) -> Option<String> {
    let is_cmd_script = matches!(bin, "npm" | "pnpm");
    // PATH 探测
    let via_path = {
        let mut c = if is_cmd_script {
            command::hidden_cmd(bin)
        } else {
            command::hidden(bin)
        };
        c.args(args);
        c.output().ok()
    };
    if let Some(out) = via_path {
        if out.status.success() {
            return parse_stdout(&out.stdout);
        }
    }
    // 回退：用户级 node_dir 内的 bin（node 为 .exe；npm/pnpm 为 .cmd，需 cmd 包装）
    let fallback = core_toolchain::node_dir().join(if is_cmd_script {
        format!("{bin}.cmd")
    } else {
        format!("{bin}.exe")
    });
    if !fallback.exists() {
        return None;
    }
    let mut c = if is_cmd_script {
        // 绝对路径 .cmd 也需 cmd /C 包装
        let mut cc = std::process::Command::new("cmd.exe");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cc.creation_flags(0x0800_0000);
        }
        cc.arg("/D").arg("/C").arg(&fallback);
        cc
    } else {
        command::hidden(&fallback)
    };
    c.args(args);
    let out = c.output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_stdout(&out.stdout)
}

/// 解析命令 stdout（trim + 非空过滤；统一解码：UTF-8 优先 + 代码页回退）
fn parse_stdout(bytes: &[u8]) -> Option<String> {
    let s = crate::core::text::decode(bytes);
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
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

/// 安装 Node：官方 zip 解压到用户级目录（免管理员）+ 写用户 PATH
fn install_node(logger: &Arc<Logger>) -> Result<String, String> {
    core_toolchain::install_node(logger)
}

/// 安装 Git：官方安装包，需提权（runas）
fn install_git(logger: &Arc<Logger>) -> Result<String, String> {
    let url = core_toolchain::git_installer_url();
    let dest = core_toolchain::git_installer_path();
    logger.progress("toolchain", InstallPhase::Download, 0, "下载 Git 安装包…");
    core_toolchain::download(logger, &url, &dest)?;
    logger.progress("toolchain", InstallPhase::Download, 100, "Git 安装包下载完成");
    logger.info("启动 Git 静默安装（请在弹出的 UAC 中确认）…");
    logger.progress("toolchain", InstallPhase::Install, 0, "等待 UAC 确认…");
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
            crate::core::text::decode(&out.stderr).trim()
        ));
    }
    logger.progress("toolchain", InstallPhase::Done, 100, "Git 安装已启动");
    Ok(format!(
        "Git 安装程序已启动（UAC 确认后完成），安装包位于 {}",
        dest.display()
    ))
}

/// 安装 pnpm：npm i -g pnpm（流式 + 进度）
/// v0.4.0：npm 不在系统 PATH 时，注入用户级 node_dir（Node 由本启动器安装后），
/// 修复分发机器"'npm' 不是内部或外部命令"问题
fn install_pnpm(logger: &Arc<Logger>) -> Result<String, String> {
    logger.info("开始安装 pnpm（npm i -g pnpm）…");
    logger.progress("toolchain", InstallPhase::Install, 0, "安装 pnpm…");
    // 定位 npm：优先系统 PATH；找不到则回退用户级 node_dir 的 npm.cmd
    let npm_in_path = command::hidden_cmd("npm")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let mut c = if npm_in_path {
        command::hidden_cmd("npm")
    } else {
        // 用户级 node_dir 内的 npm.cmd（node zip 自带）
        let fallback = core_toolchain::node_dir().join("npm.cmd");
        if !fallback.exists() {
            return Err("未找到 npm（请先安装 Node）".to_string());
        }
        command::hidden_cmd(&fallback)
    };
    // 注入 node_dir 到 PATH（npm 的子进程 node 也要能找到）
    crate::core::pathutil::inject_node_path_into(&mut c);
    c.args(["i", "-g", "pnpm"]);
    crate::core::stream::run_streamed(
        logger,
        c,
        crate::core::logging::LogLevel::Info,
        crate::core::logging::LogLevel::Warn,
        None,
    )
    .map_err(|e| format!("pnpm 安装失败: {e}"))?;
    logger.progress("toolchain", InstallPhase::Done, 100, "pnpm 安装完成");
    Ok("pnpm 安装成功".to_string())
}

/// 安装 Python：官方完整安装包静默安装（提权；写 PATH）
fn install_python(logger: &Arc<Logger>) -> Result<String, String> {
    core_toolchain::install_python(logger)
}
