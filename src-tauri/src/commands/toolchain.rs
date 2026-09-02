//! 工具链检测与一键安装 IPC
//!
//! 清单（CONTEXT.md）：Node.js(22.19+/24+)、npm、pnpm、Git(2.26+)、Python(可选)
//! 状态：present / missing / mismatch
//! 安装策略（Q4/Q15/Q16）：
//! - Node：官方 zip 解压到用户级目录（免管理员，见 core/toolchain.rs）
//! - Git：官方安装包，需提权 → spawn runas（Q22）
//! - pnpm：npm i -g
//!
//! 并发策略：detect/install 都是多进程调用 + 网络下载，全部 async + spawn_blocking，
//! 互不阻塞（各占独立后台线程）。

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
/// v0.1.7：流式日志 + 进度事件（logger 来自 AppState）
/// 完成后广播 toolchain://changed（前端实时刷新条目状态）；Git 为 UAC 异步安装，
/// 前端侧需等待安装真正结束（见 ToolchainPanel 的轮询逻辑）
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
            other => Err(format!("不支持的工具链: {other}")),
        };
        // 广播工具链变更（前端刷新条目状态 present/missing）。
        // 例外：git 为 UAC 异步安装（Start-Process 立即返回，安装未真正完成），
        // 此时广播会误报“仍缺失”；由 ToolchainPanel 显示 UAC 提示并等用户完成后再手动刷新
        if result.is_ok() && name != "git" {
            crate::core::events::emit_toolchain_changed(&app);
        }
        result
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

fn detect_node() -> ToolchainItem {
    // run_cmd 内部已处理 PATH 优先 + node_dir 回退（统一逻辑，见 run_cmd）
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

/// 解析命令 stdout（trim + 非空过滤）
fn parse_stdout(bytes: &[u8]) -> Option<String> {
    String::from_utf8(bytes.to_vec())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 安装 Node：官方 zip 解压到用户级目录（免管理员）
fn install_node(logger: &Arc<Logger>) -> Result<String, String> {
    core_toolchain::install_node(logger)
}

/// 安装 Git：官方安装包，需提权（runas）
fn install_git(logger: &Arc<Logger>) -> Result<String, String> {
    // 下载 Git 官方安装包到本地（复用 core::toolchain::download，与 Node 下载同一实现）
    let url = "https://github.com/git-for-windows/git/releases/download/v2.47.1.windows.1/Git-2.47.1-64-bit.exe";
    let dest = core_toolchain::toolchain_dir().join("Git-2.47.1-64-bit.exe");
    logger.progress("toolchain", InstallPhase::Download, 0, "下载 Git 安装包…");
    core_toolchain::download(logger, url, &dest)?;
    logger.progress("toolchain", InstallPhase::Download, 100, "Git 安装包下载完成");
    // 提权运行安装包（静默安装到 Program Files）
    // runas 需要管理员；通过 PowerShell Start-Process -Verb RunAs 触发 UAC
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
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    logger.progress("toolchain", InstallPhase::Done, 100, "Git 安装已启动");
    Ok(format!(
        "Git 安装程序已启动（UAC 确认后完成），安装包位于 {}",
        dest.display()
    ))
}

/// 安装 pnpm：npm i -g pnpm（流式 + 进度）
fn install_pnpm(logger: &Arc<Logger>) -> Result<String, String> {
    logger.info("开始安装 pnpm（npm i -g pnpm）…");
    logger.progress("toolchain", InstallPhase::Install, 0, "安装 pnpm…");
    let mut c = command::hidden_cmd("npm");
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
