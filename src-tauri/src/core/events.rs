//! Tauri 事件推送：安装进度（install://progress）+ 版本变更（version://changed）
//!
//! 设计（docs/DESIGN.md §4.4）：前端实时流式接收日志（log://line）+ 安装进度。
//! 进度事件由安装流程（npm/GitHub 通道、工具链安装）在后台线程 emit，
//! 前端 VersionPanel 订阅后展示进度条。
//!
//! 推送入口统一挂在 Logger 上（`logger.progress(...)`）：Logger 内部持有
//! AppHandle，安装流程只需持有 Arc<Logger>，无需到处传递 AppHandle。

use serde::Serialize;
use tauri::Emitter;

/// 安装进度事件名
pub const PROGRESS_EVENT: &str = "install://progress";

/// 版本变更事件名：安装/卸载完成后广播，前端刷新已安装版本与通道状态
/// （解决卸载在 StatusCard，而 VersionPanel 各自独立 state 不联动的问题）
pub const VERSION_CHANGED_EVENT: &str = "version://changed";

/// 通过 AppHandle 广播“dsh 安装版本已变更”（无 handle 时静默丢弃）
pub fn emit_version_changed(app: &tauri::AppHandle) {
    let _ = app.emit(VERSION_CHANGED_EVENT, ());
}

/// 安装阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallPhase {
    /// 准备/清理
    Prepare,
    /// 下载（git clone / 安装包下载）
    Download,
    /// 依赖安装（npm install / pnpm install）
    Install,
    /// 构建（pnpm build）
    Build,
    /// 完成
    Done,
}

impl InstallPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            InstallPhase::Prepare => "prepare",
            InstallPhase::Download => "download",
            InstallPhase::Install => "install",
            InstallPhase::Build => "build",
            InstallPhase::Done => "done",
        }
    }
}

/// 进度事件载荷
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressPayload {
    /// 通道：npm / github / toolchain
    pub channel: String,
    /// 当前阶段
    pub phase: &'static str,
    /// 0-100 整数百分比（阶段内估算或真实进度）
    pub percent: u8,
    /// 人类可读消息
    pub message: String,
}

/// 构建一个进度事件
pub fn progress(
    channel: &str,
    phase: InstallPhase,
    percent: u8,
    message: impl Into<String>,
) -> ProgressPayload {
    ProgressPayload {
        channel: channel.to_string(),
        phase: phase.as_str(),
        percent,
        message: message.into(),
    }
}

/// 通过 AppHandle 推送进度事件（无 handle 时静默丢弃）
pub fn emit(app: &tauri::AppHandle, payload: &ProgressPayload) {
    let _ = app.emit(PROGRESS_EVENT, payload.clone());
}
