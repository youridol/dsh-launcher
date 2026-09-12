//! 插件生命周期状态机（纯函数，可单测）
//!
//! 状态定义与转换规则见 ADR-0005 `State Machine` 一节。本模块不含任何 IO，
//! 只负责"由磁盘事实派生状态"与"校验动作是否合法"，供服务层与 CLI/IPC 复用。

use serde::Serialize;

/// 插件（bundle）状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginState {
    /// 依赖不存在
    Uninstalled,
    /// 依赖存在但不声明 `dsh.bundle`（普通依赖，不可启停）
    Plain,
    /// 依赖存在 + 在 bundles 中 + 至少一行启用
    Enabled,
    /// 依赖存在 + 在 bundles 中 + 全部行禁用
    Disabled,
}

impl PluginState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PluginState::Uninstalled => "uninstalled",
            PluginState::Plain => "plain",
            PluginState::Enabled => "enabled",
            PluginState::Disabled => "disabled",
        }
    }
}

/// 行级状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RowState {
    Enabled,
    Disabled,
    /// `disabled` 由 `!!js` 表达式决定，启动器拒绝覆盖
    Expression,
}

/// 用户/系统请求的动作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginAction {
    Install,
    Enable,
    Disable,
    Uninstall,
    Repair,
    Quarantine,
}

impl PluginAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            PluginAction::Install => "install",
            PluginAction::Enable => "enable",
            PluginAction::Disable => "disable",
            PluginAction::Uninstall => "uninstall",
            PluginAction::Repair => "repair",
            PluginAction::Quarantine => "quarantine",
        }
    }
}

/// 错误类别（决定 CLI 退出码与 UI 提示）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginErrorKind {
    /// 非法状态转换
    IllegalTransition,
    /// 目标不存在
    NotFound,
    /// 同包操作进行中
    Busy,
    /// 变更后校验失败（已回滚）
    VerificationFailed,
    /// 能力缺失（如无符号链接权限）
    CapabilityMissing,
    /// 受管区块冲突（marker 不成对 / 文件结构非法）
    ManagedBlockConflict,
    /// dsh 未安装或不可安全执行
    DshNotInstalled,
    /// 其它内部错误
    Internal,
}

impl PluginErrorKind {
    /// CLI 退出码（见 ADR-0005 API 一节）
    pub fn exit_code(&self) -> i32 {
        match self {
            PluginErrorKind::IllegalTransition => 2,
            PluginErrorKind::NotFound => 3,
            PluginErrorKind::Busy => 4,
            PluginErrorKind::VerificationFailed => 5,
            PluginErrorKind::CapabilityMissing => 6,
            PluginErrorKind::ManagedBlockConflict => 7,
            PluginErrorKind::DshNotInstalled => 8,
            PluginErrorKind::Internal => 1,
        }
    }
}

/// 结构化错误
#[derive(Debug, Clone)]
pub struct PluginError {
    pub kind: PluginErrorKind,
    pub message: String,
}

impl PluginError {
    pub fn new(kind: PluginErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn illegal(message: impl Into<String>) -> Self {
        Self::new(PluginErrorKind::IllegalTransition, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(PluginErrorKind::NotFound, message)
    }

    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(PluginErrorKind::Busy, message)
    }

    pub fn verification(message: impl Into<String>) -> Self {
        Self::new(PluginErrorKind::VerificationFailed, message)
    }

    pub fn dsh_missing(message: impl Into<String>) -> Self {
        Self::new(PluginErrorKind::DshNotInstalled, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(PluginErrorKind::ManagedBlockConflict, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(PluginErrorKind::Internal, message)
    }

    /// IPC/CLI 面向用户的字符串形式：`[<exit_code>] <message>`。
    ///
    /// G2（审计 §2.2 重复代码）：此前 `commands/{plugin,mcp,skill}.rs` 各自定义了一份
    /// **逐字相同**的 `format_error`。收归到错误类型自身，使其与
    /// `PluginErrorKind::exit_code()`（ADR-0005 API 的退出码契约）就近维护、不再分叉。
    ///
    /// 注意与 `Display` 的区别：`Display` 只给 `message`（供日志/`{}` 插值），
    /// 本方法额外携带退出码前缀（供前端展示与脚本化定位）。
    pub fn ipc_message(&self) -> String {
        format!("[{}] {}", self.kind.exit_code(), self.message)
    }
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for PluginError {}

/// 由磁盘事实派生插件状态。
///
/// - `dependency_present`：profile `package.json.dependencies` 是否含该包；
/// - `in_bundles`：包名是否在 `dsh.profile.bundles` 中；
/// - `declares_bundle`：包 manifest 是否声明 `dsh.bundle.patch`；
/// - `rows`：该包在 dump 中贡献的行状态。
pub fn derive_state(
    dependency_present: bool,
    in_bundles: bool,
    declares_bundle: bool,
    rows: &[RowState],
) -> PluginState {
    if !dependency_present {
        return PluginState::Uninstalled;
    }
    if !declares_bundle {
        return PluginState::Plain;
    }
    if !in_bundles {
        // 依赖在但层列表没有：dsh 下一次 `plugin` 对账会把它加回，视为"待对账的启用"
        return PluginState::Enabled;
    }
    if rows.is_empty() {
        // 声明了 bundle 但 dump 里没有行（例如 profile 未启动/行被覆盖删除）——视为启用
        return PluginState::Enabled;
    }
    if rows.iter().all(|state| *state == RowState::Disabled) {
        PluginState::Disabled
    } else {
        PluginState::Enabled
    }
}

/// 校验动作在当前状态与行状态下是否合法。
///
/// 返回 `Ok(())` 表示可以执行；`Err` 为非法转换（或需要上层先行处理的空操作语义）。
pub fn validate(action: PluginAction, state: PluginState, rows: &[RowState]) -> Result<(), PluginError> {
    match action {
        PluginAction::Install => {
            if state != PluginState::Uninstalled {
                return Err(PluginError::illegal(format!(
                    "插件已安装（当前状态 {}），install 不是合法转换；如需更新请使用 sync",
                    state.as_str()
                )));
            }
        }
        PluginAction::Enable | PluginAction::Disable => {
            match state {
                PluginState::Uninstalled => {
                    return Err(PluginError::illegal(format!(
                        "插件未安装，无法执行 {}；请先 install",
                        action.as_str()
                    )))
                }
                PluginState::Plain => {
                    return Err(PluginError::illegal(format!(
                        "该包不声明 dsh.bundle（普通依赖），没有可{}的行",
                        action.as_str()
                    )))
                }
                PluginState::Enabled | PluginState::Disabled => {}
            }
            if rows.is_empty() {
                return Err(PluginError::illegal(format!(
                    "该插件没有可{}的行（dump 中未发现其贡献行）",
                    action.as_str()
                )));
            }
            if rows.iter().all(|row| *row == RowState::Expression) {
                return Err(PluginError::illegal(
                    "该插件的全部行由 !!js 表达式控制 disabled，启动器拒绝覆盖",
                ));
            }
        }
        PluginAction::Uninstall => {
            if state == PluginState::Uninstalled {
                // 幂等空操作由服务层返回 unchanged，这里不算非法
            }
        }
        PluginAction::Repair | PluginAction::Quarantine => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_state() {
        use RowState::*;
        assert_eq!(
            derive_state(false, false, false, &[]),
            PluginState::Uninstalled
        );
        assert_eq!(derive_state(true, false, false, &[]), PluginState::Plain);
        assert_eq!(
            derive_state(true, true, true, &[Enabled, Disabled]),
            PluginState::Enabled
        );
        assert_eq!(
            derive_state(true, true, true, &[Disabled, Disabled]),
            PluginState::Disabled
        );
        // 未在 bundles 中：视为待对账的启用
        assert_eq!(
            derive_state(true, false, true, &[Disabled]),
            PluginState::Enabled
        );
        // 无行：视为启用
        assert_eq!(derive_state(true, true, true, &[]), PluginState::Enabled);
        // 表达式行不算 disabled
        assert_eq!(
            derive_state(true, true, true, &[Expression]),
            PluginState::Enabled
        );
    }

    #[test]
    fn test_validate_enable_disable_rules() {
        use RowState::*;
        // 未安装 → 非法
        assert_eq!(
            validate(PluginAction::Enable, PluginState::Uninstalled, &[]).unwrap_err().kind,
            PluginErrorKind::IllegalTransition
        );
        // 普通依赖 → 非法
        assert_eq!(
            validate(PluginAction::Disable, PluginState::Plain, &[]).unwrap_err().kind,
            PluginErrorKind::IllegalTransition
        );
        // 全表达式 → 非法
        assert_eq!(
            validate(PluginAction::Enable, PluginState::Enabled, &[Expression])
                .unwrap_err()
                .kind,
            PluginErrorKind::IllegalTransition
        );
        // 正常启停 → 合法
        assert!(validate(PluginAction::Enable, PluginState::Disabled, &[Disabled]).is_ok());
        assert!(validate(PluginAction::Disable, PluginState::Enabled, &[Enabled]).is_ok());
        // 混合行（含表达式）→ 合法（只覆盖可覆盖的行由服务层处理）
        assert!(validate(
            PluginAction::Disable,
            PluginState::Enabled,
            &[Enabled, Expression]
        )
        .is_ok());
        // 已安装再 install → 非法
        assert_eq!(
            validate(PluginAction::Install, PluginState::Enabled, &[]).unwrap_err().kind,
            PluginErrorKind::IllegalTransition
        );
        // 未安装 uninstall → 允许（幂等空操作）
        assert!(validate(PluginAction::Uninstall, PluginState::Uninstalled, &[]).is_ok());
    }

    #[test]
    fn test_exit_codes() {
        assert_eq!(PluginErrorKind::IllegalTransition.exit_code(), 2);
        assert_eq!(PluginErrorKind::NotFound.exit_code(), 3);
        assert_eq!(PluginErrorKind::Busy.exit_code(), 4);
        assert_eq!(PluginErrorKind::VerificationFailed.exit_code(), 5);
        assert_eq!(PluginErrorKind::CapabilityMissing.exit_code(), 6);
        assert_eq!(PluginErrorKind::ManagedBlockConflict.exit_code(), 7);
        assert_eq!(PluginErrorKind::DshNotInstalled.exit_code(), 8);
    }
}
