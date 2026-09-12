//! MCP Server 管理服务门面（ADR-0006 Part A）
//!
//! 官方机制（`$DSH_SRC`，逐条标注出处见各子模块）：
//! - 一个 MCP server = cordis 配置树中的一行（`name: '@deepseek-ai/dsh-mcp-client'`）；
//! - patch 层的声明必须包在 `- insert:` 里；启停是行级 `disabled` 覆盖；
//! - 机器级 `$DSH_HOME/cordis.patch.yml` 是**最后持久层**，其定向条目可命中
//!   任何更早层（bundle / profile / home 用户区）声明的行；
//! - 官方**没有**任何列 MCP 状态或列工具的 CLI，故可观测性边界见 ADR-0006 §Testing 3.3。
//!
//! 分层：`entry`（条目模型）→ `block`（受管区块读写）→ `state` / `validate` /
//! `prereq`（纯判定）→ `mod`（本文件，服务门面）；CLI 与 IPC 都是薄壳。
//!
//! 五个动作的语义（ADR §State Machine 2）：
//! - `list`：只读，覆盖合成树全量 mcp 行；
//! - `add`：`missing` → `enabled`（或 `disabled`）；不一致 → `IllegalTransition`；
//! - `enable` / `disable`：只写/改定向行；无对象 → `NotFound`；表达式行 → `IllegalTransition`；
//! - `remove`：`managed` 删声明+定向；`external` **仅删定向**（声明仍在）。
//!
//! **三类变更均不重启 dsh**：受管 profile `web` 为 `patchReload: "live"`，
//! 官方只 `watchUserPatches` profile 与 home 两个 patch 文件
//! （`apps/cli/src/profile-boot.ts:355-381`）。

pub mod block;
pub mod entry;
pub mod prereq;
pub mod state;
pub mod validate;

use crate::core::dshhome::{self, MANAGED_PROFILE};
use crate::core::logging::Logger;
use crate::core::mcp::entry::{McpBlock, McpDeclare, McpDirective, McpTransport};
use crate::core::mcp::prereq::{PrereqState, PrereqView};
use crate::core::mcp::state::{
    McpAction, McpOrigin, McpServerView, McpTransportView,
};
use crate::core::plugin::dump::{self, McpRow};
use crate::core::plugin::managed;
use crate::core::plugin::state::{PluginError, PluginErrorKind};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// `list.reload` 取值（由 profile 的 `patchReload` 决定，ADR §State Machine 2）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReloadMode {
    /// 官方注册了 watcher（`patchReload === "live"`）→ 就地热重载，不重启
    Live,
    /// 无 watcher（`patchReload === "startup"`）→ 改动需重启 dsh
    RequiresRestart,
}

/// `list` 结果（CLI `--json` 与 IPC 同一形状，见 ADR §API 2）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpListResult {
    pub profile: String,
    pub reload: ReloadMode,
    pub prereq: PrereqView,
    pub servers: Vec<McpServerView>,
}

/// 操作结果（沿用 ADR-0005 的 `OpResult` 形状；`restarted` 恒为 `false`）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpResult {
    pub status: &'static str,
    pub restarted: bool,
    pub message: String,
    pub server_name: Option<String>,
}

impl OpResult {
    fn unchanged(message: impl Into<String>, server_name: Option<String>) -> Self {
        Self {
            status: "unchanged",
            restarted: false,
            message: message.into(),
            server_name,
        }
    }

    fn changed(message: impl Into<String>, server_name: Option<String>) -> Self {
        Self {
            status: "changed",
            restarted: false,
            message: message.into(),
            server_name,
        }
    }
}

/// 结构化新增入参（CLI 结构化通道 / IPC 共用）。
///
/// **不含 `failOnStartupError`**（D10）：该字段是危险开关，结构化通道不暴露
/// （不写即官方默认 `false`）；只在 `--raw-config` 的原始文本通道中原样透传。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAddSpec {
    pub server_name: String,
    pub transport: String,
    /// 默认 `mcp-<serverName>`
    pub row_id: Option<String>,
    /// 声明为 disabled（默认 enabled）
    pub start_disabled: bool,
    // ---- stdio ----
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<String>,
    // ---- streamable-http ----
    pub url: Option<String>,
    pub headers: Vec<(String, String)>,
    // ---- 共同可选项（全部为官方字段名）----
    pub tool_call_timeout_ms: Option<u64>,
    pub reconnect_enabled: Option<bool>,
    pub reconnect_initial_delay_ms: Option<u64>,
    pub reconnect_max_delay_ms: Option<u64>,
    pub reconnect_max_attempts: Option<u64>,
    /// 原始通道：官方 `config` 体的原始 YAML 片段（与结构化字段互斥）
    pub raw_config: Option<String>,
}

/// `add` 的最小 transport 入参（校验必填字段用）
struct AddInputs<'a> {
    transport: McpTransport,
    command: Option<&'a str>,
    url: Option<&'a str>,
}

// ============================ 只读发现 ============================

/// 读取受管 MCP 区块（marker 异常 → `ManagedBlockConflict`）。
fn read_block() -> Result<McpBlock, PluginError> {
    let path = dshhome::home_patch_path();
    match block::read(&path) {
        Ok(Some(block)) => Ok(block),
        Ok(None) => Ok(McpBlock::default()),
        Err(reason) => Err(PluginError::conflict(format!("{}: {reason}", path.display()))),
    }
}

/// 只读合成树（`dsh --profile web --dump-config`，带超时、不启动插件）。
fn dump_tree() -> Result<Vec<McpRow>, PluginError> {
    let text = crate::core::profile::dump_config(MANAGED_PROFILE)?;
    Ok(dump::mcp_rows(&text))
}

/// 读取 profile 的 `patchReload`（决定 `list.reload`）。
fn reload_mode() -> ReloadMode {
    let Ok(dir) = dshhome::profile_dir(MANAGED_PROFILE) else {
        return ReloadMode::RequiresRestart;
    };
    match crate::core::profile::read_manifest(&dir) {
        Ok(manifest) if manifest.patch_reload.as_deref() == Some("live") => ReloadMode::Live,
        _ => ReloadMode::RequiresRestart,
    }
}

/// 由合成树 + 受管区块构造视图（`origin` / `marks` / 来源层）。
fn build_views(block: &McpBlock, tree: &[McpRow]) -> Vec<McpServerView> {
    let home_layer = dshhome::home_patch_path().display().to_string();
    // serverName 重复计数（冲突标记）
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for row in tree {
        if let Some(name) = row.server_name() {
            *counts.entry(name).or_insert(0) += 1;
        }
    }
    let mut views: Vec<McpServerView> = tree
        .iter()
        .filter_map(|row| {
            // 官方 `serverName` 必填；畸形行（无 serverName）不进列表，但写入仍然保真
            let server_name = row.server_name()?;
            let origin = state::derive_origin(block, &row.row_id);
            let duplicate = counts.get(&server_name).copied().unwrap_or(0) > 1;
            Some(McpServerView {
                server_name,
                row_id: row.row_id.clone(),
                transport: row.transport().map(McpTransportView::from),
                state: state::derive_state(Some(row)),
                origin,
                layer: match origin {
                    McpOrigin::Managed => home_layer.clone(),
                    McpOrigin::External => row.section_owner.clone(),
                },
                summary: row.summary(),
                disabled: state::effective_disabled(Some(row)),
                marks: state::derive_marks(row, duplicate),
            })
        })
        .collect();
    views.sort_by(|a, b| a.server_name.cmp(&b.server_name));
    views
}

/// `list`：只读发现（覆盖合成树全量 mcp 行）。
///
/// 前置缺失（`dsh-mcp-client` 不可解析）时仍返回空/完整列表 + 前置横幅 ——
/// 前置只影响 `add`，不阻塞 `list`（ADR §State Machine 3）。
pub fn list(logger: &Arc<Logger>) -> Result<McpListResult, PluginError> {
    let prereq = prereq::probe(MANAGED_PROFILE);
    if !matches!(prereq, PrereqState::Installed) {
        logger.info(&format!(
            "MCP 前置缺失：{} 不可解析（add 将被拒绝，list 仍可用）",
            prereq::REQUIRED_PACKAGE
        ));
    }
    let block = read_block()?;
    let tree = dump_tree()?;
    let servers = build_views(&block, &tree);
    Ok(McpListResult {
        profile: MANAGED_PROFILE.to_string(),
        reload: reload_mode(),
        prereq: PrereqView::from_state(&prereq),
        servers,
    })
}

/// 供 CLI / 测试复用：合成树中按 `serverName` 取行。
pub fn find_in_tree<'a>(tree: &'a [McpRow], server_name: &str) -> Option<&'a McpRow> {
    tree.iter()
        .find(|row| row.server_name().as_deref() == Some(server_name))
}

// ============================ 写入 ============================

/// 备份目录：`%LOCALAPPDATA%\dsh-launcher\backups\mcp\<serverName>\<ts>\`
fn backup_dir(server_name: &str) -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("dsh-launcher")
        .join("backups")
        .join("mcp");
    base.join(server_name).join(crate::core::profile::timestamp())
}

/// 把「块外内容」规范化成可比较的文本。
///
/// 取「受管区块之前」与「之后」两段内容，只做两件归一化：
/// - 行尾归一（CRLF → LF）：内容等价，非内容改动；
/// - 去掉 `after` 的前导换行。
///
/// 为什么不去掉 `before` 的尾换行、也不整体 `trim`：`managed::apply_body` 在
/// **追加**区块时会在块前插入一个空行分隔（避免块与上一行粘连），在**删除**区块时
/// 会把它去掉。因此"块前是否有那一个分隔换行"取决于本区块当前是否存在 —— 若整体
/// trim，删除后用户原有的尾换行会被当成"内容变化"而误报。
///
/// 于是：`before` 按原样保留（只归一化行尾），`after` 去掉前导换行（块后的分隔换行
/// 同样是产物而非用户内容）。真实的内容改动（用户手写段、其它家族区块、注释）仍会
/// 被逐字节检出。
fn canonical_outside(content: &str) -> Result<String, PluginError> {
    match managed::split_outside(content, block::MCP) {
        // 无本家族区块：整个文件都是「块外」；仍带分隔符，保证与"有区块"形态可比
        Ok(None) => Ok(format!(
            "{}\u{0}",
            normalize_eol(content).trim_end_matches('\n')
        )),
        Ok(Some((before, after))) => Ok(format!(
            "{}\u{0}{}",
            normalize_eol(&before).trim_end_matches('\n'),
            normalize_eol(&after).trim_start_matches('\n')
        )),
        Err(reason) => Err(PluginError::conflict(reason)),
    }
}

/// 行尾归一化（比较用；`.gitattributes`/编辑器可能改变 CRLF，但内容等价）
fn normalize_eol(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// 稳定的非加密哈希（FNV-1a 64 位；仅用于「内容是否变化」的等值比较）。
fn fnv1a(text: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

/// 块外内容指纹（用于「块外内容未变」断言）
fn fingerprint_outside(content: &str) -> Result<String, PluginError> {
    Ok(fnv1a(&canonical_outside(content)?))
}

/// 写后校验的目标态。
enum Expected {
    /// 受管声明的期望启停（写后该行的有效 `disabled` 必须等于它）
    RowDisabled(String, bool),
    /// 目标行必须**没有**受管定向覆盖（`managed` 真删除 / `external` 撤销覆盖）
    ///
    /// 这里**不能**断言 dump 中 `disabled == false`：官方语义是"无定向覆盖即默认启用"，
    /// 而 `--dump-config` 对未写 `disabled` 的行**不打印**该字段（有效值 = 默认启用）。
    /// 因此"恢复默认"的正确断言是「受管区块内不再有指向该行的定向条目」。
    NoDirective(String),
}

/// 写受管区块 + 校验 + 失败回滚。
///
/// 校验口径（D12 / A5）：
/// - **写入正确性** = 重新 `--dump-config` 后目标行的 `disabled` 与期望一致
///   （`--dump-config` **不启动插件**，只证明配置层，见 `apps/cli/src/dump-config.ts:1-7`）；
/// - **块外内容未变** = 块外指纹前后一致；
/// - 运行态是否真正生效**无官方 CLI 可查**，不在此断言。
fn apply_with_verification(
    server_name: &str,
    expected: Option<Expected>,
    next: &McpBlock,
    logger: &Arc<Logger>,
) -> Result<(), PluginError> {
    let path = dshhome::home_patch_path();
    let before_content = managed::read_file(&path)
        .map_err(|e| PluginError::conflict(e))?
        .unwrap_or_default();
    let before_fingerprint = fingerprint_outside(&before_content)?;

    // 先备份（目录 / 文件变更一律"先备份、后原子写"）
    let backup = backup_dir(server_name);
    if !before_content.is_empty() {
        let targets = vec![(path.clone(), PathBuf::from("cordis.patch.yml"))];
        let saved = crate::core::profile::backup_files(&targets, &backup)?;
        logger.info(&format!(
            "MCP 变更前已备份 {} 个文件到 {}",
            saved.len(),
            backup.display()
        ));
    }

    let outcome = block::apply(&path, next)
        .map_err(|e| PluginError::new(PluginErrorKind::ManagedBlockConflict, e))?;
    if outcome == managed::BlockOutcome::Unchanged {
        // 理论上不可达（服务层已判幂等）；保守返回成功
        return Ok(());
    }

    let after_content = managed::read_file(&path)
        .map_err(|e| PluginError::internal(e))?
        .unwrap_or_default();

    let mut failures: Vec<String> = Vec::new();
    // G3（审计 SEC-03②）：写后指纹**不得以 `?` 早退**。
    //
    // 旧实现为 `let after_fingerprint = fingerprint_outside(&after_content)?;`：
    // 当写入把区块写成**重复 marker**（`split_outside` → `Err`）时，该 `?` 会在进入
    // 下方回滚块之前就返回，导致「校验失败→回滚」语义被绕过，坏文件永久残留。
    // 现在把错误压入 `failures`，与其它校验失败一视同仁地进入回滚路径。
    let after_fingerprint = match fingerprint_outside(&after_content) {
        Ok(fingerprint) => Some(fingerprint),
        Err(error) => {
            failures.push(format!(
                "写后无法读取受管区块（受管区块 marker 可能已损坏）：{}",
                error.message
            ));
            None
        }
    };
    if Some(before_fingerprint.as_str()) != after_fingerprint.as_deref() {
        failures.push(format!(
            "受管 MCP 区块之外的**内容**发生变化（用户手写段 / 其它区块 / 注释被改动）"
        ));
    }
    if let Some(expected) = expected {
        match expected {
            Expected::RowDisabled(row_id, expected_disabled) => match dump_tree() {
                Ok(tree) => {
                    let actual = tree
                        .iter()
                        .find(|row| row.row_id == row_id)
                        .and_then(|row| state::effective_disabled(Some(row)));
                    if actual != Some(expected_disabled) {
                        failures.push(format!(
                            "重新 dump 后行 {row_id} 的 disabled 与期望不一致（期望 {expected_disabled}，实际 {actual:?}）"
                        ));
                    }
                }
                Err(error) => {
                    failures.push(format!("写后校验无法读取合成树：{}", error.message))
                }
            },
            Expected::NoDirective(row_id) => match read_block() {
                Ok(block) => {
                    if block.directive_by_row_id(&row_id).is_some() {
                        failures.push(format!(
                            "受管区块仍残留指向行 {row_id} 的定向覆盖（否则 dsh 每次启动都会 warn）"
                        ));
                    }
                }
                Err(error) => {
                    failures.push(format!("写后校验无法读取受管区块：{}", error.message))
                }
            },
        }
    }

    if failures.is_empty() {
        return Ok(());
    }

    // 回滚：从备份还原（失败也要如实报出）
    logger.error(&format!(
        "MCP 变更校验失败，开始回滚：{}",
        failures.join("；")
    ));
    let restored = crate::core::profile::restore_files(
        &backup,
        &[(path.clone(), PathBuf::from("cordis.patch.yml"))],
    );
    if let Err(error) = restored {
        failures.push(format!("回滚失败：{}", error.message));
    }
    Err(PluginError::verification(format!(
        "MCP 变更校验失败（已回滚）：{}",
        failures.join("；")
    )))
}

/// 由结构化入参渲染官方 `config` 体（原始文本，逐行构造，不引入 YAML 依赖）。
fn render_structured_config(spec: &McpAddSpec, transport: McpTransport) -> Result<String, PluginError> {
    let mut out = String::new();
    let quote = managed::yaml_quote;
    out.push_str(&format!("  serverName: {}\n", quote(&spec.server_name)));
    out.push_str(&format!(
        "  transport: {}\n",
        match transport {
            McpTransport::Stdio => "stdio",
            McpTransport::StreamableHttp => "streamable-http",
        }
    ));
    match transport {
        McpTransport::Stdio => {
            let command = spec.command.as_deref().unwrap_or_default().trim();
            out.push_str(&format!("  command: {}\n", quote(command)));
            if spec.args.is_empty() {
                out.push_str("  args: []\n");
            } else {
                out.push_str("  args:\n");
                for arg in &spec.args {
                    out.push_str(&format!("    - {}\n", quote(arg)));
                }
            }
            if !spec.env.is_empty() {
                out.push_str("  env:\n");
                for (key, value) in &spec.env {
                    out.push_str(&format!("    {}: {}\n", quote(key), quote(value)));
                }
            }
            if let Some(cwd) = spec.cwd.as_deref().filter(|value| !value.trim().is_empty()) {
                out.push_str(&format!("  cwd: {}\n", quote(cwd)));
            }
        }
        McpTransport::StreamableHttp => {
            let url = spec.url.as_deref().unwrap_or_default().trim();
            out.push_str(&format!("  url: {}\n", quote(url)));
            if !spec.headers.is_empty() {
                out.push_str("  headers:\n");
                for (key, value) in &spec.headers {
                    out.push_str(&format!("    {}: {}\n", quote(key), quote(value)));
                }
            }
        }
    }
    if let Some(timeout) = spec.tool_call_timeout_ms {
        out.push_str(&format!("  toolCallTimeoutMs: {timeout}\n"));
    }
    // reconnect 是官方**可选**嵌套字段（`config-catalog.md:1566-1576`），仅在有值时渲染
    let reconnect_any = spec.reconnect_enabled.is_some()
        || spec.reconnect_initial_delay_ms.is_some()
        || spec.reconnect_max_delay_ms.is_some()
        || spec.reconnect_max_attempts.is_some();
    if reconnect_any {
        out.push_str("  reconnect:\n");
        if let Some(enabled) = spec.reconnect_enabled {
            out.push_str(&format!("    enabled: {enabled}\n"));
        }
        if let Some(value) = spec.reconnect_initial_delay_ms {
            out.push_str(&format!("    initialDelayMs: {value}\n"));
        }
        if let Some(value) = spec.reconnect_max_delay_ms {
            out.push_str(&format!("    maxDelayMs: {value}\n"));
        }
        if let Some(value) = spec.reconnect_max_attempts {
            out.push_str(&format!("    maxAttempts: {value}\n"));
        }
    }
    // 危险字段 failOnStartupError **不在此暴露**（D10）
    Ok(out.trim_end().to_string())
}

/// `add`：受管区块声明段增行 + 定向段增行。
///
/// 幂等：同 `serverName` 已存在且来源与内容一致 → 服务层在 `existing` 判定处返回
/// `IllegalTransition`（ADR §State Machine 2 明确"不一致 → IllegalTransition"；
/// 一致的情形由 D9 唯一性前置拒绝，避免产生第二份声明）。
pub fn add(spec: &McpAddSpec, logger: &Arc<Logger>) -> Result<OpResult, PluginError> {
    let server_name = spec.server_name.trim().to_string();
    let transport = McpTransport::parse(spec.transport.trim()).ok_or_else(|| {
        PluginError::illegal(format!(
            "未知 transport {:?}（官方只有 stdio / streamable-http）",
            spec.transport
        ))
    })?;

    // 原始通道与结构化字段互斥（ADR §API 2）
    if spec.raw_config.is_some() {
        let has_structured = spec.command.is_some()
            || spec.url.is_some()
            || !spec.args.is_empty()
            || !spec.env.is_empty()
            || !spec.headers.is_empty()
            || spec.cwd.is_some()
            || spec.tool_call_timeout_ms.is_some()
            || spec.reconnect_enabled.is_some()
            || spec.reconnect_initial_delay_ms.is_some()
            || spec.reconnect_max_delay_ms.is_some()
            || spec.reconnect_max_attempts.is_some();
        if has_structured {
            return Err(PluginError::illegal(
                "--raw-config 与结构化字段互斥，请只保留一种（原始通道用于 !!js / 未知字段透传）",
            ));
        }
    }

    // 前置：`dsh-mcp-client` 不可解析时拒绝 add（CapabilityMissing = 6）
    if !matches!(prereq::probe(MANAGED_PROFILE), PrereqState::Installed) {
        return Err(PluginError::new(
            PluginErrorKind::CapabilityMissing,
            format!(
                "缺少前置 {}（不可解析）；请在插件页安装后再添加 MCP server",
                prereq::REQUIRED_PACKAGE
            ),
        ));
    }

    // 合成树 + 受管区块
    let tree = dump_tree()?;
    let block = read_block()?;

    let row_id = spec
        .row_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| entry::default_row_id(&server_name));

    // 官方两条校验 + transport 必填。
    // `--raw-config` 通道下 `--transport` 只作无约束提示：真实 transport 与必填字段
    // 一律以**原始 YAML 片段**为准（ADR §API 2 的"原始通道"语义）。
    let config_raw = match spec.raw_config.as_deref() {
        Some(raw) => block::normalize_config_raw(raw),
        None => render_structured_config(spec, transport)?,
    };
    let parsed_command = block::config_scalar(&config_raw, "command");
    let parsed_url = block::config_scalar(&config_raw, "url");
    let parsed_transport = block::config_scalar(&config_raw, "transport")
        .and_then(|value| McpTransport::parse(&value));
    let inputs = AddInputs {
        transport: if spec.raw_config.is_some() {
            parsed_transport.ok_or_else(|| {
                PluginError::illegal(
                    "原始 config 片段缺少官方必填字段 transport（stdio / streamable-http）",
                )
            })?
        } else {
            transport
        },
        command: parsed_command.as_deref(),
        url: parsed_url.as_deref(),
    };

    validate::validate_server_name(&server_name)?;
    validate::validate_transport_required(inputs.transport, inputs.command, inputs.url)?;
    // 唯一性：对合成树全量（排除自身行 id —— 声明尚不存在，故不排除）
    validate::validate_unique_server_name(&server_name, &tree, None)?;
    // 受管区块里可能已有同 rowId 的残留声明（例如上次 remove 未清干净）
    if let Some(existing) = block.declare_by_row_id(&row_id) {
        return Err(PluginError::illegal(format!(
            "受管区块已存在行 {row_id}（serverName {:?}），不能重复声明",
            existing.server_name
        )));
    }
    // config 的 serverName 必须与入参一致（原始通道防误配）
    if let Some(from_raw) = block::config_scalar(&config_raw, "serverName") {
        if from_raw != server_name {
            return Err(PluginError::illegal(format!(
                "config 中的 serverName（{from_raw}）与目标 serverName（{server_name}）不一致"
            )));
        }
    } else {
        return Err(PluginError::illegal(
            "config 体缺少官方必填字段 serverName",
        ));
    }

    let mut next = block.clone();
    next.declares.push(McpDeclare {
        row_id: row_id.clone(),
        server_name: Some(server_name.clone()),
        transport: Some(inputs.transport),
        config_raw,
    });
    next.directives.push(McpDirective {
        row_id: row_id.clone(),
        disabled: spec.start_disabled,
    });

    apply_with_verification(
        &server_name,
        Some(Expected::RowDisabled(row_id.clone(), spec.start_disabled)),
        &next,
        logger,
    )?;
    logger.info(&format!(
        "已添加 MCP server {server_name}（行 {row_id}，{}）",
        if spec.start_disabled {
            "声明为禁用"
        } else {
            "启用"
        }
    ));
    Ok(OpResult::changed(
        format!(
            "已添加 MCP server {server_name}（{}）",
            if spec.start_disabled {
                "已禁用"
            } else {
                "已启用"
            }
        ),
        Some(server_name),
    ))
}

/// `enable` / `disable`：只写 / 改定向行（`config` 绝不重渲染，D4）。
pub fn set_state(
    server_name: &str,
    action: McpAction,
    logger: &Arc<Logger>,
) -> Result<OpResult, PluginError> {
    let tree = dump_tree()?;
    let row = find_in_tree(&tree, server_name);
    // 无对象 → NotFound（不是幂等空操作）；表达式行 → IllegalTransition
    state::validate_transition(server_name, row, action)?;
    // G6（审计 RT-05）：不用 `expect`（生产路径不得 panic）。
    // `validate_transition` 已保证存在，但把它写成显式分支：即使未来该校验被改动，
    // 这里也只是返回 Internal 错误，而不是把整个 spawn_blocking 线程打崩。
    let Some(row) = row else {
        return Err(PluginError::internal(format!(
            "内部不一致：{server_name} 通过状态校验后目标行丢失"
        )));
    };
    let row_id = row.row_id.clone();
    let desired = action.desired_disabled();

    let block = read_block()?;
    if state::same_directive(&block, &row_id, desired) {
        return Ok(OpResult::unchanged(
            format!(
                "MCP server {server_name} 已处于{}状态，无需变更",
                if desired { "禁用" } else { "启用" }
            ),
            Some(server_name.to_string()),
        ));
    }

    let mut next = block.clone();
    match next
        .directives
        .iter_mut()
        .find(|directive| directive.row_id == row_id)
    {
        Some(directive) => directive.disabled = desired,
        None => next.directives.push(McpDirective {
            row_id: row_id.clone(),
            disabled: desired,
        }),
    }
    // 不变量 #2：定向条目必须能指向某条声明（同层或更早层）——受管声明与合成树既有行都算。
    //
    // 该不变量在此路径上**已由上游保证**，故不需要额外守卫（ADR-0009 D4，2026-09-12 清理）：
    // `row` 来自 `find_in_tree(&tree, server_name)`，其 `None` 情形已由
    // `state::validate_transition`（`core/mcp/state.rs`）以 `NotFound`(3) 拦下 ——
    // 因此能走到这里的 `row` 必然存在于合成树，`row_id` 必然指向一条真实声明。
    // 曾经的写法 `block.declare_by_row_id(&row_id).is_none() && row.row_id != row_id` 中，
    // 右项是同一变量的自比较、**恒为 false**，使整个守卫恒不可达（死代码）。

    apply_with_verification(
        server_name,
        Some(Expected::RowDisabled(row_id.clone(), desired)),
        &next,
        logger,
    )?;
    logger.info(&format!(
        "MCP server {server_name} 已{}（行 {row_id}）",
        if desired { "禁用" } else { "启用" }
    ));
    Ok(OpResult::changed(
        format!(
            "MCP server {server_name} 已{}",
            if desired { "禁用" } else { "启用" }
        ),
        Some(server_name.to_string()),
    ))
}

/// `remove`：`managed` 真删除（声明 + 定向）；`external` **仅删定向**。
///
/// 两种后果必须在消息里区分（D7），否则用户会以为删掉了服务器而其实只是撤销了覆盖。
pub fn remove(server_name: &str, logger: &Arc<Logger>) -> Result<OpResult, PluginError> {
    let tree = dump_tree()?;
    let row = find_in_tree(&tree, server_name);
    let Some(row) = row else {
        // 目标不存在 → NotFound(3)（ADR §State Machine 3）
        return Err(PluginError::not_found(format!(
            "MCP server {server_name} 不存在（合成树中没有该 serverName 的行）"
        )));
    };
    let row_id = row.row_id.clone();
    let block = read_block()?;
    let origin = state::derive_origin(&block, &row_id);

    match origin {
        McpOrigin::Managed => {
            let mut next = block.clone();
            next.declares.retain(|item| item.row_id != row_id);
            next.directives.retain(|item| item.row_id != row_id);
            if next == block {
                return Ok(OpResult::unchanged(
                    format!("MCP server {server_name} 的受管声明已不存在"),
                    Some(server_name.to_string()),
                ));
            }
            // 真删除后该行不再存在 → 无期望态可校验，只校验块外字节
            apply_with_verification(server_name, None, &next, logger)?;
            logger.info(&format!(
                "已删除受管 MCP server {server_name}（行 {row_id}：声明 + 定向一并删除）"
            ));
            Ok(OpResult::changed(
                format!("已删除受管 MCP server {server_name}（声明与定向覆盖均已移除）"),
                Some(server_name.to_string()),
            ))
        }
        McpOrigin::External => {
            let mut next = block.clone();
            next.directives.retain(|item| item.row_id != row_id);
            if next == block {
                return Ok(OpResult::unchanged(
                    format!("MCP server {server_name} 没有受管定向覆盖，无需变更"),
                    Some(server_name.to_string()),
                ));
            }
            // 撤销覆盖 → 该行恢复默认启用
            apply_with_verification(
                server_name,
                Some(Expected::NoDirective(row_id.clone())),
                &next,
                logger,
            )?;
            logger.info(&format!(
                "已撤销 MCP server {server_name} 的定向覆盖（行 {row_id}；声明仍在 {}）",
                row.section_owner
            ));
            Ok(OpResult::changed(
                format!(
                    "已撤销 MCP server {server_name} 的受管定向覆盖（声明仍由 {} 提供，服务器恢复默认启用）",
                    row.section_owner
                ),
                Some(server_name.to_string()),
            ))
        }
    }
}

/// CLI / IPC 的启停入口（动作名 → `McpAction`）。
pub fn set_enabled(
    server_name: &str,
    enabled: bool,
    logger: &Arc<Logger>,
) -> Result<OpResult, PluginError> {
    let action = if enabled {
        McpAction::Enable
    } else {
        McpAction::Disable
    };
    set_state(server_name, action, logger)
}

/// 备份根目录（供测试与文档引用）
pub fn backups_root() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("dsh-launcher")
        .join("backups")
        .join("mcp")
}

/// 读取受管区块（供调试 / 测试）
pub fn read_managed_block() -> Result<McpBlock, PluginError> {
    read_block()
}

/// 受管区块文件路径
pub fn managed_block_path() -> PathBuf {
    dshhome::home_patch_path()
}

/// 判断一个错误是否表示"受管区块冲突"（CLI 用于选择退出码 7）
pub fn is_block_conflict(error: &PluginError) -> bool {
    error.kind == PluginErrorKind::ManagedBlockConflict
}

/// 只读文件（供测试比较写入前的字节）
pub fn read_patch_file() -> Result<String, PluginError> {
    managed::read_file(&dshhome::home_patch_path())
        .map_err(|e| PluginError::internal(e))
        .map(|value| value.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER_PATCH: &str = concat!(
        "# 用户手写机器级 patch 层\n",
        "# 用户自己的注释\n",
        "\n",
        "- insert:\n",
        "    - id: mcp-github\n",
        "      name: '@deepseek-ai/dsh-mcp-client'\n",
        "      config:\n",
        "        serverName: github\n",
        "        transport: streamable-http\n",
        "        url: https://api.githubcopilot.com/mcp/\n",
        "        headers:\n",
        "          Authorization: !!js >-\n",
        "            (() => 'Bearer x')()\n",
        "        toolCallTimeoutMs: 60000\n",
        "        failOnStartupError: false\n",
    );

    const BLOCK: &str = concat!(
        "# >>> dsh-launcher mcp v1 — 由启动器维护，请勿手工编辑 >>>\n",
        "- insert:\n",
        "    - id: mcp-x\n",
        "      name: '@deepseek-ai/dsh-mcp-client'\n",
        "      config:\n",
        "        serverName: 'x'\n",
        "        transport: stdio\n",
        "        command: 'x-mcp'\n",
        "- id: mcp-x\n",
        "  disabled: false\n",
        "# <<< dsh-launcher mcp v1 <<<\n",
    );

    #[test]
    fn test_canonical_outside_ignores_block_presence_and_boundary_newlines() {
        // 无区块 = 纯用户内容
        let no_block = canonical_outside(USER_PATCH).unwrap();
        // 有区块（append 形态：用户内容 + 一个分隔换行 + 区块）
        let with_block = canonical_outside(&format!("{USER_PATCH}\n{BLOCK}")).unwrap();
        assert_eq!(
            no_block, with_block,
            "区块存在与否不得改变「块外内容」的规范化结果"
        );
        // 块后有尾部内容时也保持一致
        let with_tail = canonical_outside(&format!("{USER_PATCH}\n{BLOCK}\n# tail\n")).unwrap();
        assert_ne!(no_block, with_tail, "真实新增的尾部注释必须被检出");
        // 真的改了用户内容 → 必须被检出
        let tampered = canonical_outside(&USER_PATCH.replace("github", "github2")).unwrap();
        assert_ne!(no_block, tampered, "用户内容改动必须被检出");
        // 注释改动也必须被检出
        let comment_changed = canonical_outside(&USER_PATCH.replace("用户自己的注释", "改过了")).unwrap();
        assert_ne!(no_block, comment_changed, "注释改动必须被检出");
    }

    #[test]
    fn test_canonical_outside_tolerates_crlf() {
        let lf = canonical_outside(USER_PATCH).unwrap();
        let crlf = canonical_outside(&USER_PATCH.replace('\n', "\r\n")).unwrap();
        assert_eq!(lf, crlf);
    }
}
