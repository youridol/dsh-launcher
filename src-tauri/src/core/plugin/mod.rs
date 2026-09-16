//! 插件管理服务：状态机 + 受管区块 + 官方 API 调用 + 幂等/回滚
//!
//! 对外只暴露四个动作：`list` / `install` / `set_state` / `uninstall`，外加
//! `repair`（收敛）与 `sync`（upstream 自动同步，见 sync.rs）。
//! 所有动作都遵循 ADR-0005 的语义：
//! - 非法转换直接拒绝（`IllegalTransition`）；
//! - 期望态与磁盘一致时返回 `unchanged`，不落盘、不重启、不发事件；
//! - 任何失败都从备份回滚并返回 `VerificationFailed`/`Internal`。

pub mod dump;
pub mod managed;
pub mod registry;
pub mod spec;
pub mod state;
pub mod sync;

use crate::core::dshhome;
use crate::core::events::InstallPhase;
use crate::core::logging::Logger;
use crate::core::process::{DshStatus, ProcessManager};
use crate::core::profile;
use crate::core::stream;
use dump::DumpSection;
use managed::ManagedEntry;
use registry::{PluginRecord, PluginSource, Registry};
use serde::Serialize;
use spec::Origin;
use state::{PluginAction, PluginError, PluginErrorKind, PluginState, RowState};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// 不可卸载的模板层包（profile 模板的第一层，见 app-boot/profile.ts）
pub const PROTECTED_PACKAGES: [&str; 2] = ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"];

/// 操作结果状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OpStatus {
    Changed,
    Unchanged,
}

/// 单个动作的结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpResult {
    pub status: OpStatus,
    pub restarted: bool,
    pub message: String,
    pub package: Option<String>,
}

impl OpResult {
    fn unchanged(message: impl Into<String>, package: Option<String>) -> Self {
        Self {
            status: OpStatus::Unchanged,
            restarted: false,
            message: message.into(),
            package,
        }
    }

    fn changed(message: impl Into<String>, restarted: bool, package: Option<String>) -> Self {
        Self {
            status: OpStatus::Changed,
            restarted,
            message: message.into(),
            package,
        }
    }
}

/// 行视图
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowView {
    pub id: String,
    pub name: Option<String>,
    pub state: RowState,
}

/// 插件视图
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginView {
    pub package: String,
    pub version: Option<String>,
    pub origin: Origin,
    pub state: PluginState,
    pub rows: Vec<RowView>,
    pub source: Option<PluginSource>,
    pub last_sync: Option<registry::SyncRecord>,
    pub protected: bool,
    pub needs_reconcile: bool,
    pub desired: Option<String>,
    pub last_error: Option<String>,
    pub installed_spec: Option<String>,
}

impl PluginView {
    /// 来源标签（CLI 展示用）
    pub fn origin_str(&self) -> &'static str {
        match self.origin {
            Origin::Upstream => "upstream",
            Origin::InHouse => "in-house",
            Origin::Unknown => "unknown",
        }
    }
}

/// 列表结果（`degraded_reason` 非空表示 dump 解析失败，启停按钮应禁用）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginList {
    pub profile: String,
    pub plugins: Vec<PluginView>,
    pub degraded_reason: Option<String>,
}

/// 同步报告
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub applied: bool,
    pub restarted: bool,
    pub items: Vec<SyncItemResult>,
}

/// 单个插件的同步结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncItemResult {
    pub package: String,
    pub origin: Origin,
    pub from: Option<String>,
    pub to: Option<String>,
    pub result: String,
    pub message: String,
}

// ============================ 并发互斥 ============================

/// 同步任务的保留键（同步进行中禁止任何其它插件操作）
const SYNC_KEY: &str = "__sync__";

fn in_flight() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 在飞操作守卫（Drop 时自动释放）
#[derive(Debug)]
struct OpGuard {
    key: String,
}

impl OpGuard {
    /// 申请一个操作键；同键或同步进行中 → `Busy`。
    fn acquire(key: &str) -> Result<Self, PluginError> {
        let mut set = in_flight().lock().unwrap_or_else(|e| e.into_inner());
        if set.contains(SYNC_KEY) {
            return Err(PluginError::busy("同步任务进行中，请稍后再试"));
        }
        if key != SYNC_KEY && set.contains(key) {
            return Err(PluginError::busy(format!("插件 {key} 正在处理中")));
        }
        if key == SYNC_KEY && !set.is_empty() {
            return Err(PluginError::busy("已有插件操作进行中，请稍后再同步"));
        }
        set.insert(key.to_string());
        Ok(Self {
            key: key.to_string(),
        })
    }
}

impl Drop for OpGuard {
    fn drop(&mut self) {
        let mut set = in_flight().lock().unwrap_or_else(|e| e.into_inner());
        set.remove(&self.key);
    }
}

// ============================ 只读发现 ============================

/// patch 文件结构体检（BUG-2，v0.9.8）：把「dsh 无法解析该文件」这一事实
/// 转成可操作的诊断。
///
/// 背景（2026-09-16 实测）：cordis.patch.yml 一旦被外部工具写坏（重复内容、
/// 顶层不是数组、marker 不成对），dsh 的 dump/启动都会整体失败，而启动器此前
/// 只会显示 "dsh --dump-config 失败（退出码 1）" + 大段 Node 堆栈，用户无从下手；
/// 且损坏文件**无法经 UI 自愈**（apply_block 对非法顶层拒绝写入）。
///
/// 本函数只做两件事（都只读，不修改文件）：
/// 1. 结构体检：marker 是否成对、顶层是否是数组（`[]` / 块序列 / 纯注释），
///    以及文件是否出现「同一段模板头重复」这类典型误写；
/// 2. 产出 human-readable 原因，供 UI/日志展示。
/// @returns None = 文件不存在或结构可信；Some(原因) = 结构异常及说明
pub fn inspect_patch_file(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    if content.trim().is_empty() {
        return None;
    }
    // 1) marker 成对性（与 apply_body 的 locate 语义一致）
    let begin_count = content.matches(&managed::mark_begin()).count();
    let end_count = content.matches(&managed::mark_end()).count();
    if begin_count != end_count {
        return Some(format!(
            "受管区块 marker 不成对（起始 {begin_count} 个 / 结束 {end_count} 个）"
        ));
    }
    if begin_count > 1 {
        return Some(format!(
            "受管区块 marker 重复（起始 {begin_count} 个，应为 1）——文件可能被重复写入或外部工具拼接损坏"
        ));
    }
    // 2) 顶层必须是 YAML 数组：`[]` 占位符 / 块序列 / 纯注释（与 apply_body 判定一致）
    let has_array_placeholder = content
        .lines()
        .any(|line| line.trim() == "[]");
    let looks_like_block_sequence = content.lines().all(|line| {
        let trimmed = line.trim_start();
        trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("- ")
            || line.starts_with(' ')
            || line.starts_with('\t')
    });
    if !has_array_placeholder && !looks_like_block_sequence {
        // 找第一行"看起来不是注释/空行/序列项"的内容，作为定位提示
        let offender = content
            .lines()
            .enumerate()
            .find(|(_, line)| {
                let trimmed = line.trim();
                !trimmed.is_empty()
                    && !trimmed.starts_with('#')
                    && !trimmed.starts_with("- ")
                    && !line.starts_with(' ')
                    && !line.starts_with('\t')
            })
            .map(|(index, line)| format!("第 {} 行: {}", index + 1, line.trim()))
            .unwrap_or_else(|| "未知位置".to_string());
        return Some(format!(
            "顶层不是 YAML 数组（既不是 `[]` 也不是块序列）：{offender}——文件可能被外部工具写坏"
        ));
    }
    // 3) 典型误写：同一段模板头出现两次（外部脚本拼接损坏）
    let head_signature = "# Your patch layer for this dsh profile";
    if content.matches(head_signature).count() > 1 {
        return Some(format!(
            "模板头重复出现 {} 次——文件被重复拼接（外部工具写坏）",
            content.matches(head_signature).count()
        ));
    }
    None
}

/// patch 文件自愈（BUG-2，v0.9.8）：当文件被写坏、dsh 无法解析时，把文件恢复成
/// dsh 可接受的最小合法形态，**并保留用户数据**。
///
/// 恢复策略（保守，逐级回退）：
/// 1. 备份原文件到 `cordis.patch.yml.corrupt-<timestamp>`（绝不静默丢弃数据）；
/// 2. 若受管区块的 marker 成对，提取区块内容 → 重建「模板头 + 区块」；
/// 3. 若无法提取，退化为「模板头 + `[]`」（用户非受管内容由备份保全）。
/// @returns 恢复后的内容摘要（成功）或错误
pub fn heal_patch_file(path: &std::path::Path) -> Result<String, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let backup = path.with_extension(format!(
        "yml.corrupt-{}",
        profile::timestamp().replace([':', ' '], "-")
    ));
    std::fs::write(&backup, &content)
        .map_err(|e| format!("备份 {} 失败: {e}", backup.display()))?;

    // 受管区块原文（marker 成对且内容非空时提取）
    let block_body = match managed::read_body(path, managed::MANAGED) {
        Ok(Some(body)) if !body.trim().is_empty() => Some(body),
        _ => None,
    };

    // 块外用户数据抢救（数据保护）：收集「合法 YAML 行」并去重。
    // 判据：非空、非注释、且形如 `- ...` 或缩进行——这正是用户/插件的行；
    // 损坏残留（如裸文本 `rofile, applied ...`、重复的模板头注释）天然被排除。
    let mut user_lines: Vec<String> = Vec::new();
    let outside = managed::split_outside(&content, managed::MANAGED).ok().flatten();
    let (before_text, after_text) = match &outside {
        Some((before, after)) => (before.as_str(), after.as_str()),
        None => (content.as_str(), ""),
    };
    for line in before_text.lines().chain(after_text.lines()) {
        let trimmed_start = line.trim_start();
        if trimmed_start.is_empty() || trimmed_start.starts_with('#') {
            continue;
        }
        let is_yaml_row = trimmed_start.starts_with("- ")
            || line.starts_with(' ')
            || line.starts_with('\t');
        if is_yaml_row && !user_lines.iter().any(|existing| existing == line) {
            user_lines.push(line.to_string());
        }
    }

    // 重建：模板头（`[]` 占位按需替换）+ 抢救出的用户行 +（可选）受管区块
    let template = PROFILE_PATCH_TEMPLATE.trim_end_matches(|c| c == '\n');
    let template_without_placeholder = template
        .strip_suffix("[]")
        .map(|head| head.trim_end())
        .unwrap_or(template);
    let mut out = String::new();
    if user_lines.is_empty() && block_body.is_none() {
        // 无任何内容：保持标准模板（含 `[]` 占位）
        out.push_str(PROFILE_PATCH_TEMPLATE);
    } else {
        out.push_str(template_without_placeholder);
        out.push('\n');
        for line in &user_lines {
            out.push_str(line);
            out.push('\n');
        }
        if let Some(body) = &block_body {
            out.push_str(&managed::render_block_body(body));
            out.push('\n');
        }
    }
    std::fs::write(path, &out).map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    Ok(format!(
        "已修复 {}（原文件备份到 {}；保留 {} 行用户内容{}）",
        path.display(),
        backup.display(),
        user_lines.len(),
        if block_body.is_some() { "与受管区块" } else { "" }
    ))
}

/// profile patch 文件的标准模板头（与 dsh `initProfile` 写入的模板一致）。
const PROFILE_PATCH_TEMPLATE: &str = "# Your patch layer for this dsh profile, applied after every bundle layer:
# a top-level YAML array of loader patch entries (id-targeted config
# overrides, disables, and insert lists; `!!js` expressions allowed).
# []
";


/// 读取 profile 依赖 + bundles + dump 段。
struct Discovered {
    profile_dir: PathBuf,
    manifest: profile::ProfileManifest,
    sections: Vec<DumpSection>,
    degraded_reason: Option<String>,
}

fn discover(profile_name: &str, logger: &Arc<Logger>) -> Result<Discovered, PluginError> {
    let dir = profile::profile_dir(profile_name)?;
    if !dir.join("package.json").exists() {
        return Err(PluginError::not_found(format!(
            "profile {profile_name} 尚未初始化（{} 不存在）；请先启动一次 dsh 或安装插件",
            dir.display()
        )));
    }
    let manifest = profile::read_manifest(&dir)?;
    let (sections, degraded_reason) = match profile::dump_config(profile_name)
        .and_then(|text| dump::parse_dump(&text).map_err(|e| PluginError::internal(e)))
    {
        Ok(sections) => (sections, None),
        // dsh 不可用是"能力缺失"，不是可降级情形：直接上抛（CLI 退出码 8）
        Err(error) if error.kind == PluginErrorKind::DshNotInstalled => return Err(error),
        Err(error) => {
            // BUG-2（v0.9.8）：dump 失败时先对 profile patch 做结构体检。
            // 若病根是 patch 文件损坏，给出可操作诊断（而不是只丢 Node 堆栈），
            // 并提示「一键修复」入口（heal_patch_file）；这能把用户从
            // "插件面板逐个禁用排查" 的错误方向拉回真正的病根。
            let patch_path = dshhome::profile_patch_path(profile_name).ok();
            let diagnose = patch_path
                .as_deref()
                .and_then(inspect_patch_file)
                .map(|reason| {
                    format!(
                        "profile patch 文件结构损坏（{reason}）。dsh 因此无法启动/枚举插件。                         可在本面板点击「修复配置文件」（自动备份原文件后重建），或手工修复后重试。                         原始错误：{}",
                        error.message
                    )
                });
            let message = diagnose.unwrap_or_else(|| error.message.clone());
            logger.warn(&format!(
                "解析 dsh --dump-config 失败，插件列表降级为只读：{message}"
            ));
            (Vec::new(), Some(message))
        }
    };
    Ok(Discovered {
        profile_dir: dir,
        manifest,
        sections,
        degraded_reason,
    })
}

/// 由磁盘事实构建插件视图（纯函数，便于单测）。
fn build_views(
    profile_dir: &std::path::Path,
    manifest: &profile::ProfileManifest,
    sections: &[DumpSection],
    registry: &Registry,
) -> Vec<PluginView> {
    let index = dump::index_by_id(sections);
    let mut views: Vec<PluginView> = Vec::new();

    for (package, spec_value) in &manifest.dependencies {
        let in_bundles = manifest.bundles.iter().any(|item| item == package);
        let declares = profile::declares_bundle(package, profile_dir);
        let row_ids = dump::rows_of_owner(sections, package);
        let rows: Vec<RowView> = row_ids
            .iter()
            .filter_map(|id| index.get(id))
            .map(|(_, row)| RowView {
                id: row.id.clone(),
                name: row.name.clone(),
                state: match row.effective_enabled() {
                    Some(true) => RowState::Enabled,
                    Some(false) => RowState::Disabled,
                    None => RowState::Expression,
                },
            })
            .collect();
        let row_states: Vec<RowState> = rows.iter().map(|row| row.state).collect();
        let plugin_state = state::derive_state(true, in_bundles, declares, &row_states);
        let record = registry.find(package);
        views.push(PluginView {
            package: package.clone(),
            version: profile::package_version(package, profile_dir),
            origin: record.map(|item| item.origin).unwrap_or_else(|| spec::classify(spec_value).1),
            state: plugin_state,
            rows,
            source: record.and_then(|item| item.source.clone()),
            last_sync: record.and_then(|item| item.last_sync.clone()),
            protected: record.map(|item| item.protected).unwrap_or(false)
                || PROTECTED_PACKAGES.contains(&package.as_str()),
            needs_reconcile: declares && !in_bundles,
            desired: record.and_then(|item| item.desired.clone()),
            last_error: record.and_then(|item| item.last_error.clone()),
            installed_spec: Some(spec_value.clone()),
        });
    }
    views.sort_by(|a, b| a.package.cmp(&b.package));
    views
}

/// 首次运行时从磁盘引导注册表（只读 profile，仅写 `plugins.json`）。
///
/// 引导阶段**不触碰 profile 任何文件**（ADR-0005 Migration 一节）：只把
/// 依赖集合、来源分类、行集合、受保护标记落进注册表。
/// @returns 是否发生变更（需要保存）
fn bootstrap_registry(
    registry: &mut Registry,
    manifest: &profile::ProfileManifest,
    sections: &[DumpSection],
    logger: &Arc<Logger>,
) -> bool {
    let mut changed = false;
    for (package, spec_value) in &manifest.dependencies {
        if registry.find(package).is_some() {
            continue;
        }
        let (kind, origin) = spec::classify(spec_value);
        let mut record = PluginRecord::new(package.clone());
        record.origin = origin;
        let (repo, reference) = spec::git_repo_and_ref(spec_value)
            .map(|(repo, reference)| (Some(repo), reference))
            .unwrap_or((None, None));
        record.source = Some(PluginSource {
            kind,
            spec: spec::full_spec(package, spec_value),
            repo,
            reference,
            commit: spec::pinned_commit(spec_value),
        });
        record.rows = dump::rows_of_owner(sections, package);
        record.protected = PROTECTED_PACKAGES.contains(&package.as_str());
        logger.info(&format!(
            "插件注册表引导：{package}（来源 {origin:?}，spec {spec_value}）"
        ));
        registry.upsert(record);
        changed = true;
    }
    // 受保护标记修正（即使是既有记录）
    for record in registry.plugins.iter_mut() {
        if PROTECTED_PACKAGES.contains(&record.package.as_str()) && !record.protected {
            record.protected = true;
            changed = true;
        }
    }
    changed
}

/// 列出受管 profile 的插件（磁盘为事实源）。
pub fn list(profile_name: &str, logger: &Arc<Logger>) -> Result<PluginList, PluginError> {
    let mut registry = Registry::load(profile_name);
    let discovered = discover(profile_name, logger)?;
    let mut dirty =
        bootstrap_registry(&mut registry, &discovered.manifest, &discovered.sections, logger);
    let views = build_views(
        &discovered.profile_dir,
        &discovered.manifest,
        &discovered.sections,
        &registry,
    );
    // 清理手工删除后残留的注册表记录（磁盘为事实源）
    let known: HashSet<&str> = discovered
        .manifest
        .dependencies
        .keys()
        .map(String::as_str)
        .collect();
    let before = registry.plugins.len();
    registry.plugins.retain(|item| known.contains(item.package.as_str()));
    if registry.plugins.len() != before {
        dirty = true;
    }
    if dirty {
        if let Err(e) = registry.save() {
            logger.warn(&format!("保存插件注册表失败: {e}"));
        }
    }
    Ok(PluginList {
        profile: profile_name.to_string(),
        plugins: views,
        degraded_reason: discovered.degraded_reason,
    })
}

/// 读取某插件的行状态（用于动作校验）。
fn rows_for(discovered: &Discovered, package: &str) -> (Vec<String>, Vec<RowState>) {
    let index = dump::index_by_id(&discovered.sections);
    let ids = dump::rows_of_owner(&discovered.sections, package);
    let states = ids
        .iter()
        .filter_map(|id| index.get(id))
        .map(|(_, row)| match row.effective_enabled() {
            Some(true) => RowState::Enabled,
            Some(false) => RowState::Disabled,
            None => RowState::Expression,
        })
        .collect();
    (ids, states)
}

/// 影子恢复行判定（v0.9.7，修复 2026-09-16 故障）：
///
/// 部分插件采用「禁用官方行 + 子类替换」模式——插件 bundle patch 把官方行
/// （如 @michengai/dsh-archive-manager@0.1.43 禁用 @deepseek-ai/dsh-web-app 的
/// `workspace` 行）置 disabled，再 insert 同服务的子类行。此时启动器「禁用该插件」
/// 只禁子类行的话，官方行仍是 disabled → 服务无人提供 → 下游全部 pending
///（实测：workspaceRegistry pending → session/workspace-controller、ui-git-graph、
/// ui-task-board、ui-deliverables 全链瘫痪，Sessions/工作区不可访问）。
///
/// 修复语义：
/// - **disable** 插件 X：除禁用 X 的行外，对所有「被 X 替换的官方行」写受管
///   `disabled: false`（恢复官方行兜底）；
/// - **enable** 插件 X：移除这些官方行的受管条目（子类 patch 自然重新接管），
///   并保留子类禁用行的移除。
///
/// 「被 X 替换的官方行」判定（基于 dump，不猜语义）：
/// 1. 行 Y 所在段头为 `A, patched by ..., X, ...`（X 参与覆盖了该段）；
/// 2. Y 属于**官方 bundle 层**（owner 是 bundle，非用户 patch 路径）；
/// 3. Y.effective_enabled() == Some(false)（当前被禁用）；
/// 4. Y.id 不在 X 自己拥有的行里（否则是 X 对自己行的改动，不属于替换）；
/// 5. X 自己拥有至少一行（rows_of_owner 非空——有子类才有「替换」可言）。
///
/// 表达式控制的行跳过（启动器不覆盖表达式，与受管区块既有约束一致）。
/// 恢复动作写入受管区块，enable 时自动移除 → 完全可回滚。
fn shadow_restore_rows(discovered: &Discovered, package: &str) -> Vec<String> {
    let own_rows = dump::rows_of_owner(&discovered.sections, package);
    if own_rows.is_empty() {
        return Vec::new();
    }
    let own: std::collections::BTreeSet<&str> =
        own_rows.iter().map(|s| s.as_str()).collect();
    let mut restored = Vec::new();
    for section in &discovered.sections {
        // 段头必须明确列出本插件参与覆盖（"A, patched by ..., <package>, ..."）
        if !section.patched_by.iter().any(|who| who == package) {
            continue;
        }
        // 只处理官方 bundle 层：用户 patch 层 owner 是绝对文件路径
        //（Windows 形如 C:\Users\...，含盘符冒号+反斜杠；bundle 包名如
        // @scope/pkg 只含正斜杠，绝不包含反斜杠或盘符冒号模式）。
        let owner = &section.owner;
        let is_user_patch_layer = owner.contains('\\')
            || (owner.len() >= 2 && owner.as_bytes()[1] == b':')
            || owner.starts_with('/');
        if is_user_patch_layer {
            continue;
        }
        for row in &section.rows {
            if own.contains(row.id.as_str()) {
                continue;
            }
            if row.is_expression_controlled() {
                continue;
            }
            if row.effective_enabled() == Some(false) {
                restored.push(row.id.clone());
            }
        }
    }
    restored.sort();
    restored.dedup();
    restored
}

// ============================ 启停 ============================

/// 启用/禁用插件（写受管区块，dsh 热重载，不重启）。
pub fn set_state(
    profile_name: &str,
    package: &str,
    enabled: bool,
    logger: &Arc<Logger>,
) -> Result<OpResult, PluginError> {
    let _guard = OpGuard::acquire(package)?;
    set_state_inner(profile_name, package, enabled, logger)
}

/// 无锁版启停（调用方必须已持有相应操作键；`repair` 复用）。
fn set_state_inner(
    profile_name: &str,
    package: &str,
    enabled: bool,
    logger: &Arc<Logger>,
) -> Result<OpResult, PluginError> {
    let action = if enabled {
        PluginAction::Enable
    } else {
        PluginAction::Disable
    };
    let mut registry = Registry::load(profile_name);
    let discovered = discover(profile_name, logger)?;
    if let Some(reason) = &discovered.degraded_reason {
        return Err(PluginError::internal(format!(
            "无法解析 dsh 配置树（{reason}），启停已禁用"
        )));
    }
    if !discovered.manifest.dependencies.contains_key(package) {
        return Err(PluginError::illegal(format!(
            "插件 {package} 未安装，无法执行 {}",
            action.as_str()
        )));
    }
    let in_bundles = discovered.manifest.bundles.iter().any(|item| item == package);
    let declares = profile::declares_bundle(package, &discovered.profile_dir);
    let (row_ids, row_states) = rows_for(&discovered, package);
    let current = state::derive_state(true, in_bundles, declares, &row_states);
    state::validate(action, current, &row_states)?;

    // 受保护包同样允许启停（只是不允许卸载）
    let patch_path = dshhome::profile_patch_path(profile_name)
        .map_err(|e| PluginError::internal(e))?;
    let existing = managed::read_block(&patch_path).map_err(|e| {
        PluginError::new(PluginErrorKind::ManagedBlockConflict, e)
    })?;
    let updates: Vec<ManagedEntry> = row_ids
        .iter()
        .zip(row_states.iter())
        .filter(|(_, row_state)| **row_state != RowState::Expression)
        .map(|(id, _)| ManagedEntry::new(id.clone(), !enabled, Some(package.to_string())))
        .collect();
    if updates.is_empty() {
        return Err(PluginError::illegal(format!(
            "插件 {package} 的行全部由表达式控制，启动器拒绝覆盖"
        )));
    }
    // 影子恢复行（v0.9.7，修复「插件禁用官方行+子类替换」模式下的孤儿服务）：
    // disable → 对被本插件替换的官方行写启用行（保证服务有人提供），
    //           并把恢复清单记入注册表（shadow_restored）；
    // enable → 按注册表记录移除这些官方行的受管启用条目（子类 patch 重新接管），
    //           并清空记录。按记录而非重新计算：enable 后官方行已是 enabled，
    //           重新计算会得到空清单导致启用行残留（实测缺陷）。
    let record = registry.plugins.iter().find(|r| r.package == package);
    let remembered_shadow: Vec<String> = record
        .map(|r| r.shadow_restored.clone())
        .unwrap_or_default();
    // BUG-1（v0.9.8 修复）：记忆与本次扫描结果取【并集】，不得被空扫描覆盖。
    //
    // 触发序列（2026-09-16 实测）：disable → disable → enable。
    // 第 1 次 disable：扫描到官方行（disabled）→ 写受管启用行 + 记忆 [workspace]。
    // 第 2 次 disable：官方行已 enabled（被受管块覆盖）→ 扫描返回**空** →
    //   若用空覆盖记忆，则 enable 时 remove 集合为空 → 受管启用行残留 →
    //   官方行与插件子类同时启用 → cordis 报 service 重复注册 → 插件无法激活。
    // 并集语义同时也覆盖 repair/sync 重放期望态（重复 disable）与 live 重载。
    let (shadow_updates, remove_ids, shadow_to_remember): (Vec<ManagedEntry>, Vec<String>, Vec<String>) =
        if enabled {
            (
                Vec::new(),
                remembered_shadow.clone(),
                Vec::new(),
            )
        } else {
            let mut ids = shadow_restore_rows(&discovered, package);
            for id in &remembered_shadow {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
            ids.sort();
            ids.dedup();
            (
                ids.iter()
                    .map(|id| ManagedEntry::new(id.clone(), false, Some(package.to_string())))
                    .collect(),
                Vec::new(),
                ids,
            )
        };
    let mut all_updates = updates.clone();
    all_updates.extend(shadow_updates.iter().cloned());
    let desired_entries = managed::upsert(existing.clone(), &all_updates, &remove_ids);
    let outcome = managed::apply_block(&patch_path, &desired_entries).map_err(|e| {
        PluginError::new(PluginErrorKind::ManagedBlockConflict, e)
    })?;
    if outcome == managed::BlockOutcome::Unchanged {
        if let Some(record) = registry
            .plugins
            .iter_mut()
            .find(|record| record.package == package)
        {
            record.rows = row_ids;
            record.shadow_restored = shadow_to_remember;
        }
        registry.set_desired(package, Some(if enabled { "enabled" } else { "disabled" }));
        let _ = registry.save();
        return Ok(OpResult::unchanged(
            format!("{package} 已处于{}状态（无变更）", if enabled { "启用" } else { "禁用" }),
            Some(package.to_string()),
        ));
    }

    // 写后校验：重新 dump 并核对目标行（含影子恢复行）
    let verify = (|| -> Result<(), PluginError> {
        let text = profile::dump_config(profile_name)?;
        let sections = dump::parse_dump(&text).map_err(|e| PluginError::internal(e))?;
        let index = dump::index_by_id(&sections);
        for entry in &updates {
            let Some((_, row)) = index.get(&entry.id) else {
                return Err(PluginError::verification(format!(
                    "校验失败：行 {} 在 dump 中不存在",
                    entry.id
                )));
            };
            match row.effective_enabled() {
                Some(value) if value == enabled => {}
                _ => {
                    return Err(PluginError::verification(format!(
                        "校验失败：行 {} 的 disabled 未按期望生效",
                        entry.id
                    )))
                }
            }
        }
        // 影子恢复行（仅 disable 分支存在）：官方行必须已恢复启用
        for entry in &shadow_updates {
            let Some((_, row)) = index.get(&entry.id) else {
                return Err(PluginError::verification(format!(
                    "校验失败：影子恢复行 {} 在 dump 中不存在",
                    entry.id
                )));
            };
            if row.effective_enabled() != Some(true) {
                return Err(PluginError::verification(format!(
                    "校验失败：影子恢复行 {} 未恢复启用",
                    entry.id
                )));
            }
        }
        Ok(())
    })();
    if let Err(error) = verify {
        // 回滚受管区块
        let rollback = match existing {
            Some(entries) => managed::apply_block(&patch_path, &entries),
            None => managed::apply_block(&patch_path, &[]),
        };
        if let Err(e) = rollback {
            logger.error(&format!("回滚受管区块失败: {e}"));
        }
        return Err(error);
    }

    if let Some(record) = registry
        .plugins
        .iter_mut()
        .find(|record| record.package == package)
    {
        record.rows = row_ids;
        record.shadow_restored = shadow_to_remember;
    }
    registry.set_desired(package, Some(if enabled { "enabled" } else { "disabled" }));
    registry.set_error(package, None);
    if let Err(e) = registry.save() {
        logger.warn(&format!("保存插件注册表失败: {e}"));
    }
    let message = format!(
        "{package} 已{}（{} 行；dsh live 热重载，无需重启）",
        if enabled { "启用" } else { "禁用" },
        updates.len()
    );
    logger.info(&message);
    Ok(OpResult::changed(message, false, Some(package.to_string())))
}

// ============================ 安装 / 卸载 ============================

/// 把相对本地路径 spec 锚定到当前工作目录（对齐 dsh `anchorPathSpec` 的语义）。
fn anchor_spec(spec: &str) -> String {
    let (kind, _) = spec::classify(spec);
    if kind != spec::SpecKind::Path {
        return spec.to_string();
    }
    let (prefix, path) = match spec.split_once(':') {
        Some((head, tail))
            if head.eq_ignore_ascii_case("file") || head.eq_ignore_ascii_case("link") =>
        {
            (Some(head.to_string()), tail.to_string())
        }
        _ => (None, spec.to_string()),
    };
    let is_relative = path.starts_with("./")
        || path.starts_with("../")
        || path.starts_with(".\\")
        || path.starts_with("..\\");
    if !is_relative {
        return spec.to_string();
    }
    let Ok(cwd) = std::env::current_dir() else {
        return spec.to_string();
    };
    let joined = cwd.join(path).to_string_lossy().to_string();
    match prefix {
        Some(prefix) => format!("{prefix}:{joined}"),
        None => joined,
    }
}

/// 备份清单：受变更影响的 profile 文件
///（`(绝对路径, 备份子路径)` —— 子路径参与文件名，见 `profile::backup_files`）。
fn backup_targets(profile_dir: &std::path::Path) -> Vec<(PathBuf, PathBuf)> {
    vec![
        (profile_dir.join("package.json"), PathBuf::from("package.json")),
        (
            profile_dir.join("pnpm-lock.yaml"),
            PathBuf::from("pnpm-lock.yaml"),
        ),
        (
            profile_dir.join("pnpm-workspace.yaml"),
            PathBuf::from("pnpm-workspace.yaml"),
        ),
        (
            profile_dir.join("cordis.patch.yml"),
            PathBuf::from("cordis.patch.yml"),
        ),
    ]
}

fn backup_dir(package: &str) -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("dsh-launcher")
        .join("backups")
        .join("plugins");
    base.join(package).join(profile::timestamp())
}

/// **唯一**的失败回滚写入点（ADR-0006 D18）。
///
/// 为什么必须有这个例外：官方没有"回退到任意历史 lock 状态"的能力 ——
/// `dsh plugin` / pnpm 只能前进（安装/更新），无法把 `package.json` +
/// `pnpm-lock.yaml` + `pnpm-workspace.yaml` 还原到某个历史一致状态。
/// 因此当官方通道执行**失败**时，启动器用备份还原这三个文件（含
/// `cordis.patch.yml`），**并立刻跟一次官方通道 `dsh plugin … install` 收敛**，
/// 让 node_modules 与还原后的 manifest 重新对齐。
///
/// ADR-0006 D18 把这条路径登记为启动器白名单的**唯一例外**：除本函数外，
/// `src-tauri/src/**` 中不存在任何写这三个 profile 文件的调用点。
/// 该不变量由 `tests/part_b_compliance_test.rs` 的静态断言守护（ADR-0009 D19 订正：
/// 此前此处引用的是并不存在的 `tests/plugin_whitelist_test.rs`）。
/// 从备份还原 profile 文件，并紧随一次官方通道收敛（ADR-0005 D18）。
///
/// BUG-2（审计）：返回**是否完全还原**。调用方（安装/卸载失败路径）据此决定
/// 要不要重启 dsh —— 完全还原就无需重启（避免无谓地中断会话 + 误导性日志）。
fn rollback_after_failed_official_op(
    profile_name: &str,
    backup: &std::path::Path,
    targets: &[(PathBuf, PathBuf)],
    logger: &Arc<Logger>,
) -> bool {
    let mut restored_ok = true;
    match profile::restore_files(backup, targets) {
        Ok(restored) => logger.warn(&format!(
            "已从备份还原 {} 个 profile 文件（官方无回退能力，D18 唯一例外）",
            restored.len()
        )),
        Err(error) => {
            restored_ok = false;
            logger.error(&format!("从备份还原 profile 文件失败: {error}"));
        }
    }
    // 紧随官方通道收敛（D18 强制）：让 node_modules 与还原后的 manifest 一致
    if let Err(error) = run_plugin_forward(profile_name, &["install".to_string()], logger) {
        logger.warn(&format!("回滚后的官方通道收敛失败（请手动执行 dsh plugin install）: {error}"));
    }
    restored_ok
}

/// 停止 dsh（返回是否曾运行）。
fn stop_if_running(
    process: Option<&Arc<ProcessManager>>,
    logger: &Arc<Logger>,
) -> bool {
    let Some(process) = process else {
        return false;
    };
    let status = process.status();
    if matches!(status, DshStatus::Running | DshStatus::Starting) {
        if let Err(e) = process.stop() {
            logger.warn(&format!("停止 dsh 失败（继续执行插件操作）: {e}"));
        }
        return true;
    }
    false
}

/// 重新启动 dsh（best-effort）。
fn restart_if_needed(
    process: Option<&Arc<ProcessManager>>,
    logger: &Arc<Logger>,
    was_running: bool,
) -> bool {
    if !was_running {
        return false;
    }
    let Some(process) = process else {
        return false;
    };
    let port = crate::core::config::AppConfig::load().port;
    match process.start(port) {
        Ok(()) => {
            logger.info(&format!("插件变更后已重新启动 dsh（端口 {port}）"));
            true
        }
        Err(e) => {
            logger.warn(&format!(
                "插件变更后自动启动 dsh 失败（请手动启动）: {e}"
            ));
            false
        }
    }
}

/// 以流式方式运行 `dsh plugin --profile <p> <args...>`。
///
/// BUG-3b（审计）：若用户配置了 npm 镜像源，且本次操作会访问 registry，
/// 则额外追加 pnpm 原生的 `--registry <url>`。
///
/// 为何用这种方式（而非自己写 .npmrc）：`dsh plugin` 就是 pnpm 的**薄转发器**
/// （官方 `apps/cli/src/plugin.ts:120-163`：`spawnSync('pnpm', args, { cwd: profileDir })`，
/// 所有 args 原样透传），因此附赠 pnpm 参数是**官方开放的**注入通道；
/// 不触碰 profile 目录的 pnpm-workspace.yaml / .npmrc，也就不违反
/// ADR-0005「白名单唯一例外」的约束。
fn run_plugin_forward(
    profile_name: &str,
    pnpm_args: &[String],
    logger: &Arc<Logger>,
) -> Result<(), PluginError> {
    let mut args = vec![
        "plugin".to_string(),
        "--profile".to_string(),
        profile_name.to_string(),
    ];
    args.extend(pnpm_args.iter().cloned());
    // 镜像注入：仅当（a）用户配了 registry 且（b）本操作确实会拉 npm 包。
    // 纯 git/路径依赖不访问 registry，加 --registry 反而多余。
    if let Some(registry) = registry_for_args(pnpm_args) {
        logger.info(&format!("使用配置的 npm 镜像源：{registry}"));
        args.push("--registry".to_string());
        args.push(registry);
    }
    let cmd = profile::build_dsh_command(&args)?;
    let logger_cb = Arc::clone(logger);
    let callback: Arc<stream::LineCallback> = Arc::new(move |_level, line| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        // pnpm 输出密集：按行推进度，95% 封顶，完成后置 100
        logger_cb.progress(
            "plugin",
            InstallPhase::Install,
            95,
            trimmed.chars().take(200).collect::<String>(),
        );
    });
    // BUG-1（审计）：改用 checked 变体，拿回 stderr 尾部 —— 让失败原因能上抛给用户。
    stream::run_streamed_checked(
        logger,
        cmd,
        crate::core::logging::LogLevel::Info,
        crate::core::logging::LogLevel::Warn,
        Some(callback),
    )
    .map_err(|failure| PluginError::internal(format!("dsh plugin 执行失败: {}", describe_pnpm_failure(&failure))))
}

/// 从配置取 npm 镜像源（仅当本次 pnpm 参数会访问 registry 时）。
///
/// - `add` 带 npm/git/tarball spec：只有 **npm 包名**才需要 registry；
/// - `install`（无 spec，如回滚后收敛）：会照 lockfile 拉包，需按需走 registry；
/// - 用户未配置镜像 → 返回 None（走官方 registry，行为不变）。
fn registry_for_args(pnpm_args: &[String]) -> Option<String> {
    let Some(registry) = crate::core::config::current_npm_registry() else {
        return None;
    };
    if !registry_should_apply(pnpm_args) {
        return None;
    }
    Some(registry)
}

/// 纯函数：本次 pnpm 参数是否需要访问 npm registry。
///
/// 抽为纯函数以便单测直接覆盖（不依赖全局配置）。
fn registry_should_apply(pnpm_args: &[String]) -> bool {
    let Some(subcommand) = pnpm_args.first().map(String::as_str) else {
        return false;
    };
    match subcommand {
        // 无 spec 的 install：按 lockfile 拉包，需 registry
        "install" | "i" => true,
        "add" => {
            // 有任一 npm 形态 spec 才需要；纯 git/路径/tarball 不需要
            pnpm_args[1..].iter().any(|arg| {
                !arg.starts_with('-')
                    && !arg.starts_with("--registry")
                    && spec::classify(arg).0 == spec::SpecKind::Npm
            })
        }
        // remove / why / list 等不注入（remove 不访问网络）
        _ => false,
    }
}

/// 把 pnpm 失败翻译成**可自助定位**的说明（BUG-1）。
///
/// 依据：`dsh plugin` 是 pnpm 的**薄转发器**（官方 `apps/cli/src/plugin.ts:120-163`
/// 的 `spawnSync('pnpm', args, { cwd: profileDir })`），所以 pnpm 的原始 stderr
/// 就是权威诊断。此前启动器把它丢弃成「命令退出码: 1」，用户无从下手。
///
/// 这里保留退出码与 stderr 尾部，并对已知的高频失败给出**指向性提示**；
/// 未识别的情形原样透传（不猜测、不编造）。
fn describe_pnpm_failure(failure: &stream::StreamFailure) -> String {
    // 关键词匹配用合并后的诊断（stdout + stderr）—— pnpm 把错误写在 stdout。
    let joined = failure.diagnostics();
    let mut detail = failure.detail();

    // ① 包不存在 / 无权限（ERR_PNPM_FETCH_404）—— 最常见的输错包名情形。
    if joined.contains("ERR_PNPM_FETCH_404") || joined.contains("is not in the npm registry") {
        detail.push_str(
            "\n提示：该 npm 包在 registry 中不存在（或名称拼写有误 / 无访问权限）。\n  ",
        );
        detail.push_str(
            "请核对包名；若想装 GitHub 仓库，请改用 git 形态（建议钉 commit）：\n  ",
        );
        detail.push_str("  https://github.com/<owner>/<repo>.git#<commit>\n  ");
        detail.push_str("  或 github:<owner>/<repo>#<commit>");
    }
    // ② pnpm ≥10 拦下 git 依赖的构建（prepare）脚本。
    //   实测错误码：`ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED`（git 源）
    //   或 `Ignored build scripts` / `allowBuilds`（registry 源）。
    //   官方 dsh CLI 对此有同样的指导（`apps/cli/src/plugin.ts:151-160`）。
    if joined.contains("allowBuilds")
        || joined.contains("Ignored build scripts")
        || joined.contains("PREPARE_NOT_ALLOWED")
        || joined.contains("needs to execute build scripts")
    {
        detail.push_str(
            "\n提示：pnpm 阻止了该依赖的构建脚本（build/prepare），安装未完成。\n  ",
        );
        detail.push_str(
            "请按上方 pnpm 打印的**确切键名**，在 profile 的 pnpm-workspace.yaml 的 allowBuilds 下\n  ",
        );
        detail.push_str("将其设为 true，然后重试。该文件路径见上方 “profile directory”。");
    }
    // ③ 网络 / registry 不可达。
    if joined.contains("ERR_PNPM_FETCH") && !joined.contains("ERR_PNPM_FETCH_404") {
        detail.push_str("\n提示：拉取失败，请检查网络或在设置中配置 npm 镜像源后重试。");
    }
    detail
}

/// 安装插件。
pub fn install(
    profile_name: &str,
    raw_spec: &str,
    origin_override: Option<Origin>,
    logger: &Arc<Logger>,
    process: Option<&Arc<ProcessManager>>,
) -> Result<OpResult, PluginError> {
    let spec_value = anchor_spec(raw_spec.trim());
    if spec_value.is_empty() {
        return Err(PluginError::illegal("安装 spec 不能为空"));
    }
    let package_hint = spec::package_name_from_spec(&spec_value)
        .unwrap_or_else(|| spec_value.clone());
    let _guard = OpGuard::acquire(&format!("install:{package_hint}"))?;

    let dir = profile::profile_dir(profile_name)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| PluginError::internal(format!("创建 profile 目录失败: {e}")))?;
    let before_deps: BTreeMap<String, String> = std::fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| value.get("dependencies").cloned())
        .and_then(|value| value.as_object().cloned())
        .map(|map| {
            map.into_iter()
                .filter_map(|(key, item)| item.as_str().map(|s| (key, s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    let backup = backup_dir(&package_hint);
    let targets = backup_targets(&dir);
    let saved = profile::backup_files(&targets, &backup)?;
    logger.info(&format!(
        "安装插件 {spec_value}（备份 {} 个文件到 {}）",
        saved.len(),
        backup.display()
    ));

    let was_running = stop_if_running(process, logger);
    logger.progress("plugin", InstallPhase::Prepare, 5, "准备安装插件…");
    let args = vec!["add".to_string(), spec_value.clone()];
    if let Err(error) = run_plugin_forward(profile_name, &args, logger) {
        logger.error(&format!("插件安装失败，开始回滚：{error}"));
        // BUG-2（审计）：安装失败且已回滚时**不重启** dsh。
        // 回滚已把 profile 文件还原到操作前状态，dsh 本来就未带这个包启动过，
        // 重启毫无收益，却会：中断用户会话、并在日志里留下一串「dsh 已启动」
        // 让人误以为安装成功。仅当回滚**未能完全还原**时，才需要重启以回到
        // 磁盘上的真实状态。
        let rollback_ok = rollback_after_failed_official_op(profile_name, &backup, &targets, logger);
        if !rollback_ok {
            logger.warn("回滚未完全还原，重启 dsh 以对齐磁盘状态…");
            restart_if_needed(process, logger, was_running);
        } else if was_running {
            logger.info("回滚完成，dsh 保持未运行状态（未做无谓重启）");
        }
        return Err(PluginError::verification(format!(
            "插件安装失败（已回滚）: {error}"
        )));
    }

    // 依赖前后 diff 得到真实包名（git/路径 spec 的依赖名以包自身 name 为准）
    let manifest = profile::read_manifest(&dir)?;
    let added: Vec<String> = manifest
        .dependencies
        .keys()
        .filter(|key| !before_deps.contains_key(*key))
        .cloned()
        .collect();
    let package = added
        .first()
        .cloned()
        .unwrap_or_else(|| package_hint.clone());

    // 校验：依赖存在；声明 bundle 时必须在 bundles 中
    let declares = profile::declares_bundle(&package, &dir);
    let in_bundles = manifest.bundles.iter().any(|item| item == &package);
    if declares && !in_bundles {
        logger.warn(&format!(
            "{package} 声明了 dsh.bundle 但未进入 dsh.profile.bundles，尝试重新对账"
        ));
        let _ = run_plugin_forward(profile_name, &["install".to_string()], logger);
        let after = profile::read_manifest(&dir)?;
        if !after.bundles.iter().any(|item| item == &package) {
            rollback_after_failed_official_op(profile_name, &backup, &targets, logger);
            restart_if_needed(process, logger, was_running);
            return Err(PluginError::verification(format!(
                "{package} 未被 dsh 对账进 dsh.profile.bundles（已回滚）"
            )));
        }
    }

    let (kind, classified_origin) = spec::classify(&spec_value);
    let origin = origin_override.unwrap_or(classified_origin);
    let (repo, reference) = spec::git_repo_and_ref(&spec_value)
        .map(|(repo, reference)| (Some(repo), reference))
        .unwrap_or((None, None));
    let commit = spec::pinned_commit(&spec_value);

    let mut registry = Registry::load(profile_name);
    let mut record = PluginRecord::new(package.clone());
    record.origin = origin;
    record.source = Some(PluginSource {
        kind,
        spec: spec_value.clone(),
        repo,
        reference,
        commit,
    });
    record.protected = PROTECTED_PACKAGES.contains(&package.as_str());
    record.desired = None;
    registry.upsert(record);
    if let Err(e) = registry.save() {
        logger.warn(&format!("保存插件注册表失败: {e}"));
    }

    logger.progress("plugin", InstallPhase::Done, 100, "插件安装完成");
    let restarted = restart_if_needed(process, logger, was_running);
    let message = format!(
        "已安装 {package}（{}）{}",
        if declares { "bundle 层已激活" } else { "普通依赖，不贡献层" },
        if restarted { "；dsh 已重启" } else { "" }
    );
    logger.info(&message);
    Ok(OpResult::changed(message, restarted, Some(package)))
}

/// 卸载插件（幂等：未安装 → unchanged）。
pub fn uninstall(
    profile_name: &str,
    package: &str,
    logger: &Arc<Logger>,
    process: Option<&Arc<ProcessManager>>,
) -> Result<OpResult, PluginError> {
    if PROTECTED_PACKAGES.contains(&package) {
        return Err(PluginError::illegal(format!(
            "{package} 是 profile 模板层，禁止卸载"
        )));
    }
    let _guard = OpGuard::acquire(package)?;
    let dir = profile::profile_dir(profile_name)?;
    if !dir.join("package.json").exists() {
        return Ok(OpResult::unchanged(
            format!("profile {profile_name} 未初始化，{package} 视为未安装"),
            Some(package.to_string()),
        ));
    }
    let manifest = profile::read_manifest(&dir)?;
    let patch_path =
        dshhome::profile_patch_path(profile_name).map_err(|e| PluginError::internal(e))?;
    let existing = managed::read_block(&patch_path)
        .map_err(|e| PluginError::new(PluginErrorKind::ManagedBlockConflict, e))?;
    let mut registry = Registry::load(profile_name);

    if !manifest.dependencies.contains_key(package) {
        // 未安装：只清理该包残留的受管条目（其它包的条目必须保留）
        let rows = registry
            .find(package)
            .map(|record| record.rows.clone())
            .unwrap_or_default();
        if !rows.is_empty() {
            let pruned = managed::upsert(existing, &[], &rows);
            let _ = managed::apply_block(&patch_path, &pruned);
        }
        registry.remove(package);
        let _ = registry.save();
        return Ok(OpResult::unchanged(
            format!("{package} 已处于未安装状态"),
            Some(package.to_string()),
        ));
    }

    // 卸载前先记录该包贡献的行（dump 为准，注册表兜底）
    let mut rows = match discover(profile_name, logger) {
        Ok(discovered) => dump::rows_of_owner(&discovered.sections, package),
        Err(_) => Vec::new(),
    };
    if rows.is_empty() {
        rows = registry
            .find(package)
            .map(|record| record.rows.clone())
            .unwrap_or_default();
    }

    let backup = backup_dir(package);
    let targets = backup_targets(&dir);
    let saved = profile::backup_files(&targets, &backup)?;
    logger.info(&format!(
        "卸载插件 {package}（备份 {} 个文件到 {}）",
        saved.len(),
        backup.display()
    ));
    let was_running = stop_if_running(process, logger);
    let args = vec!["remove".to_string(), package.to_string()];
    if let Err(error) = run_plugin_forward(profile_name, &args, logger) {
        logger.error(&format!("插件卸载失败，开始回滚：{error}"));
        rollback_after_failed_official_op(profile_name, &backup, &targets, logger);
        restart_if_needed(process, logger, was_running);
        return Err(PluginError::verification(format!(
            "插件卸载失败（已回滚）: {error}"
        )));
    }

    // 校验：依赖与层都已移除
    let after = profile::read_manifest(&dir)?;
    if after.dependencies.contains_key(package)
        || after.bundles.iter().any(|item| item == package)
    {
        rollback_after_failed_official_op(profile_name, &backup, &targets, logger);
        restart_if_needed(process, logger, was_running);
        return Err(PluginError::verification(format!(
            "{package} 仍留在 profile 依赖或 bundles 中（已回滚）"
        )));
    }

    // 清理该插件的受管条目（避免 dsh 每次启动 warn "entry not found"）
    if !rows.is_empty() {
        let pruned = managed::upsert(existing, &[], &rows);
        if let Err(e) = managed::apply_block(&patch_path, &pruned) {
            logger.warn(&format!("清理受管区块失败: {e}"));
        }
    }
    registry.remove(package);
    if let Err(e) = registry.save() {
        logger.warn(&format!("保存插件注册表失败: {e}"));
    }

    let restarted = restart_if_needed(process, logger, was_running);
    let message = format!(
        "已卸载 {package}{}",
        if restarted { "；dsh 已重启" } else { "" }
    );
    logger.info(&message);
    Ok(OpResult::changed(message, restarted, Some(package.to_string())))
}

/// 收敛：重新对账 bundles，并把注册表中的期望态重新应用一遍。
pub fn repair(
    profile_name: &str,
    only_package: Option<&str>,
    logger: &Arc<Logger>,
    process: Option<&Arc<ProcessManager>>,
) -> Result<OpResult, PluginError> {
    let key = only_package.unwrap_or(SYNC_KEY);
    let _guard = OpGuard::acquire(key)?;
    let was_running = stop_if_running(process, logger);
    logger.info("开始收敛插件状态（dsh plugin install 对账 bundles）…");
    run_plugin_forward(profile_name, &["install".to_string()], logger)?;

    let registry = Registry::load(profile_name);
    let mut changed = 0usize;
    for record in &registry.plugins {
        if let Some(only) = only_package {
            if record.package != only {
                continue;
            }
        }
        let Some(desired) = &record.desired else {
            continue;
        };
        let enabled = desired == "enabled";
        // 逐包重放期望态；失败只记录，不中断（此处已在 repair 的操作键内，走无锁版）
        match set_state_inner(profile_name, &record.package, enabled, logger) {
            Ok(result) => {
                if result.status == OpStatus::Changed {
                    changed += 1;
                }
            }
            Err(error) => {
                logger.warn(&format!(
                    "重放 {} 的期望态失败: {error}",
                    record.package
                ));
            }
        }
    }
    let restarted = restart_if_needed(process, logger, was_running);
    let message = format!("收敛完成（重放 {changed} 个插件的期望态）");
    logger.info(&message);
    if changed == 0 && !restarted {
        Ok(OpResult::unchanged(message, None))
    } else {
        Ok(OpResult::changed(message, restarted, None))
    }
}

/// 启动崩溃归因与自动隔离（取代硬编码的 dshmarket 卸载，ADR-0005 D12）。
///
/// 策略：读 dsh stderr 尾部 → 在注册表里找名字出现在 stderr 中的**非受保护**插件 →
/// 禁用其行（可逆、可热重载）并标记 `quarantine`；找不到就只给出排查指引，
/// **绝不自动卸载任何插件**。
/// @returns 被隔离的包名（未命中返回 None）
pub fn handle_boot_failure(logger: &Arc<Logger>) -> Option<String> {
    let stderr = std::fs::read_to_string(crate::core::process::dsh_stderr_path())
        .unwrap_or_default();
    // 只取尾部 32KB，避免大文件拖慢启动路径
    let tail: String = {
        let chars: Vec<char> = stderr.chars().collect();
        let start = chars.len().saturating_sub(32 * 1024);
        chars[start..].iter().collect()
    };
    if tail.trim().is_empty() {
        logger.warn("dsh stderr 为空，无法归因启动失败原因");
        return None;
    }
    // BUG-3（v0.9.8）：配置文件解析失败 ≠ 插件不兼容。
    // 此前 dsh 因 cordis.patch.yml 损坏而启动失败时，这里仍做插件名匹配，
    // 匹配不到就报「请在插件面板逐个禁用排查」——把用户引向完全错误的方向
    //（2026-09-16 实测：用户照着提示禁用插件一小时无果，真实病根是配置文件）。
    // 现在先识别解析类错误，直接给出精确诊断与修复入口。
    if tail.contains("failed to parse overlay") || tail.contains("failed to parse") {
        let patch_hint = dshhome::profile_patch_path(dshhome::MANAGED_PROFILE)
            .ok()
            .and_then(|path| {
                inspect_patch_file(&path).map(|reason| (path, reason))
            });
        match patch_hint {
            Some((path, reason)) => {
                logger.error(&format!(
                    "启动失败：profile 配置文件结构损坏（{reason}）；文件 {}。                     请在插件面板点击「修复配置文件」（自动备份后重建），或手工修复后重试。                     这不是插件不兼容，无需逐个禁用插件。",
                    path.display()
                ));
            }
            None => {
                logger.error(
                    "启动失败：dsh 报告配置文件解析错误（failed to parse）。                     请检查 ~/.dsh/profiles/web/cordis.patch.yml 与 ~/.dsh/cordis.patch.yml 的 YAML 结构；                     这不是插件不兼容，无需逐个禁用插件。",
                );
            }
        }
        return None;
    }
    let registry = Registry::load(dshhome::MANAGED_PROFILE);
    let mut matched: Option<String> = None;
    'outer: for record in &registry.plugins {
        if record.protected {
            continue;
        }
        let candidates = std::iter::once(record.package.clone()).chain(record.rows.iter().cloned());
        for candidate in candidates {
            if candidate.len() >= 4 && tail.contains(&candidate) {
                matched = Some(record.package.clone());
                break 'outer;
            }
        }
    }
    let package = matched?;
    logger.warn(&format!(
        "启动失败：stderr 命中插件 {package}，尝试隔离（禁用其行）…"
    ));
    match set_state(dshhome::MANAGED_PROFILE, &package, false, logger) {
        Ok(_) => {
            let mut registry = Registry::load(dshhome::MANAGED_PROFILE);
            if let Some(record) = registry
                .plugins
                .iter_mut()
                .find(|record| record.package == package)
            {
                record.quarantine = true;
            }
            if let Err(e) = registry.save() {
                logger.warn(&format!("保存插件注册表失败: {e}"));
            }
            Some(package)
        }
        Err(error) => {
            logger.warn(&format!("隔离 {package} 失败: {error}"));
            None
        }
    }
}

/// upstream 插件同步（`apply=false` 时只做检查与计划）。
///
/// 自研（in-house）与未知来源的包**永不进入执行**；每个包独立校验与记录，
/// 单包失败不中断批次。
pub fn sync(
    profile_name: &str,
    apply: bool,
    only_package: Option<&str>,
    logger: &Arc<Logger>,
    process: Option<&Arc<ProcessManager>>,
) -> Result<SyncReport, PluginError> {
    let _guard = OpGuard::acquire(SYNC_KEY)?;
    let dir = profile::profile_dir(profile_name)?;
    if !dir.join("package.json").exists() {
        return Err(PluginError::not_found(format!(
            "profile {profile_name} 尚未初始化，无法同步插件"
        )));
    }
    let manifest = profile::read_manifest(&dir)?;
    let mut registry = Registry::load(profile_name);
    // 同步前先引导注册表（首次运行即可自动跟踪上游）
    match discover(profile_name, logger) {
        Ok(discovered) => {
            if bootstrap_registry(
                &mut registry,
                &manifest,
                &discovered.sections,
                logger,
            ) {
                if let Err(e) = registry.save() {
                    logger.warn(&format!("保存插件注册表失败: {e}"));
                }
            }
        }
        Err(error) => logger.warn(&format!("同步前读取配置树失败（跳过引导）：{error}")),
    }
    let mut records: Vec<PluginRecord> = registry
        .plugins
        .iter()
        .filter(|record| manifest.dependencies.contains_key(&record.package))
        .filter(|record| only_package.map_or(true, |only| record.package == only))
        .cloned()
        .collect();
    records.sort_by(|a, b| a.package.cmp(&b.package));

    let version_of = |package: &str| profile::package_version(package, &dir);
    let warn = |message: &str| logger.warn(message);
    let plan_items = sync::plan(&records, version_of, &warn);
    let actionable: Vec<&sync::SyncPlanItem> =
        plan_items.iter().filter(|item| item.actionable).collect();

    if !apply {
        return Ok(SyncReport {
            applied: false,
            restarted: false,
            items: plan_items
                .iter()
                .map(|item| SyncItemResult {
                    package: item.package.clone(),
                    origin: item.origin,
                    from: item.current.clone(),
                    to: item.target.clone(),
                    result: if item.actionable { "pending" } else { "skipped" }.to_string(),
                    message: item.reason.clone(),
                })
                .collect(),
        });
    }
    if actionable.is_empty() {
        return Ok(SyncReport {
            applied: false,
            restarted: false,
            items: plan_items
                .iter()
                .map(|item| SyncItemResult {
                    package: item.package.clone(),
                    origin: item.origin,
                    from: item.current.clone(),
                    to: item.target.clone(),
                    result: "skipped".to_string(),
                    message: item.reason.clone(),
                })
                .collect(),
        });
    }

    let was_running = stop_if_running(process, logger);
    let mut results: Vec<SyncItemResult> = Vec::new();

    for item in &plan_items {
        if !item.actionable {
            results.push(SyncItemResult {
                package: item.package.clone(),
                origin: item.origin,
                from: item.current.clone(),
                to: item.target.clone(),
                result: "skipped".to_string(),
                message: item.reason.clone(),
            });
            continue;
        }
        let Some(new_spec) = item.new_spec.clone() else {
            results.push(SyncItemResult {
                package: item.package.clone(),
                origin: item.origin,
                from: item.current.clone(),
                to: item.target.clone(),
                result: "skipped".to_string(),
                message: "无可写入的目标 spec".to_string(),
            });
            continue;
        };
        logger.info(&format!("同步 {} → {}", item.package, new_spec));
        let args = vec!["add".to_string(), new_spec.clone()];
        match run_plugin_forward(profile_name, &args, logger) {
            Err(error) => {
                registry.set_error(&item.package, Some(error.message.clone()));
                results.push(SyncItemResult {
                    package: item.package.clone(),
                    origin: item.origin,
                    from: item.current.clone(),
                    to: item.target.clone(),
                    result: "failed".to_string(),
                    message: format!("安装失败: {error}"),
                });
                continue;
            }
            Ok(()) => {}
        }
        // 逐包校验
        let verified = if item.kind == spec::SpecKind::Npm {
            profile::package_version(&item.package, &dir)
        } else {
            spec::pinned_commit(&new_spec)
        };
        if verified.as_deref() != item.target.as_deref() {
            let message = format!(
                "校验失败：期望 {:?}，实际 {:?}",
                item.target, verified
            );
            registry.set_error(&item.package, Some(message.clone()));
            results.push(SyncItemResult {
                package: item.package.clone(),
                origin: item.origin,
                from: item.current.clone(),
                to: item.target.clone(),
                result: "failed".to_string(),
                message,
            });
            continue;
        }
        if let Some(record) = registry
            .plugins
            .iter_mut()
            .find(|record| record.package == item.package)
        {
            if let Some(source) = record.source.as_mut() {
                source.spec = new_spec.clone();
                if item.kind == spec::SpecKind::Git {
                    source.commit = item.target.clone();
                }
            }
            record.last_sync = Some(registry::SyncRecord {
                at: profile::timestamp(),
                from: item.current.clone().unwrap_or_default(),
                to: item.target.clone().unwrap_or_default(),
                result: "ok".to_string(),
            });
            record.last_error = None;
        }
        results.push(SyncItemResult {
            package: item.package.clone(),
            origin: item.origin,
            from: item.current.clone(),
            to: item.target.clone(),
            result: "ok".to_string(),
            message: item.reason.clone(),
        });
    }
    if let Err(e) = registry.save() {
        logger.warn(&format!("保存插件注册表失败: {e}"));
    }
    // v0.9.6（预防计划 P1-2）：存在失败项时不自动重启。
    // 失败 = profile 处于半更新状态（部分包已换版、部分失败回滚），此时重启 dsh
    // 会让它带着不一致的依赖闭包启动 —— 2026-09-16 审计案例（dsh 0.1.6 升级后
    // workspace 行 pending → Sessions/工作区不可访问）的诱因之一。让用户在全部
    // 重试成功（或显式收敛）后再启动，风险面更小。
    let failed = results.iter().filter(|item| item.result == "failed").count();
    let restarted = if failed > 0 {
        logger.warn(&format!(
            "同步存在 {failed} 个失败项，跳过自动重启 dsh（避免以半更新状态启动）；请在插件面板重试失败项或执行「收敛」，成功后再手动启动"
        ));
        false
    } else {
        restart_if_needed(process, logger, was_running)
    };
    Ok(SyncReport {
        applied: true,
        restarted,
        items: results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BUG-3b（审计）：镜像注入的适用性判定（不依赖真实全局配置）。
    #[test]
    fn test_registry_args_applicability() {
        // 纯 git 源 add → 不需 registry
        assert!(!registry_should_apply(&[
            "add".to_string(),
            "https://github.com/dsh-market/dsh-market".to_string()
        ]));
        assert!(!registry_should_apply(&[
            "add".to_string(),
            "github:o/r#abc1234".to_string()
        ]));
        // 本地路径 / tarball → 不需 registry
        assert!(!registry_should_apply(&[
            "add".to_string(),
            "link:../p".to_string()
        ]));
        // npm 包名 → 需 registry
        assert!(registry_should_apply(&[
            "add".to_string(),
            "dshmarket".to_string()
        ]));
        assert!(registry_should_apply(&[
            "add".to_string(),
            "@scope/pkg@^1.0.0".to_string()
        ]));
        // install（无 spec，按 lockfile 拉包）→ 需 registry
        assert!(registry_should_apply(&["install".to_string()]));
        // remove 等其他命令 → 不注入
        assert!(!registry_should_apply(&[
            "remove".to_string(),
            "dshmarket".to_string()
        ]));
    }

    /// BUG-1（审计）：pnpm 失败描述应保留退出码 + stderr 尾部，并对 404 给提示。
    #[test]
    fn test_describe_pnpm_failure_404_hint() {
        // pnpm 把错误写在 **stdout**（本仓实测），故此处按真实行为构造
        let failure = stream::StreamFailure {
            exit_code: Some(1),
            stdout_tail: vec![
                "[ERR_PNPM_FETCH_404] GET https://registry.npmjs.org/dsh-market: Not Found - 404"
                    .to_string(),
                "dsh-market is not in the npm registry, or you have no permission to fetch it."
                    .to_string(),
            ],
            ..Default::default()
        };
        let text = describe_pnpm_failure(&failure);
        assert!(text.contains("命令退出码 1"), "{text}");
        assert!(text.contains("ERR_PNPM_FETCH_404"), "应保留原始 stderr：{text}");
        assert!(text.contains("不存在"), "应给出 404 提示：{text}");
        assert!(text.contains("github.com"), "应给出 git 形态建议：{text}");
    }

    /// BUG-1/URL 形态：pnpm ≥10 拦下 git 依赖的构建脚本（实测错误码）。
    #[test]
    fn test_describe_pnpm_failure_allow_builds_hint() {
        let failure = stream::StreamFailure {
            exit_code: Some(1),
            stdout_tail: vec![
                "[ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED] Failed to prepare git-hosted package fetched from \"https://codeload.github.com/dsh-market/dsh-market/tar.gz/efce445\": The git-hosted package \"dshmarket@1.45.1\" needs to execute build scripts but is not in the \"allowBuilds\" allowlist."
                    .to_string(),
                "Add the package to \"allowBuilds\" in your project's pnpm-workspace.yaml to allow it to run scripts."
                    .to_string(),
            ],
            ..Default::default()
        };
        let text = describe_pnpm_failure(&failure);
        assert!(text.contains("ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED"), "{text}");
        assert!(text.contains("allowBuilds"), "应给出 allowBuilds 指引：{text}");
    }

    /// 诊断展示应过滤 dsh 自身的尾部噪声行（否则会挤掉 pnpm 的真实原因）。
    #[test]
    fn test_describe_pnpm_failure_filters_dsh_trailer() {
        let failure = stream::StreamFailure {
            exit_code: Some(1),
            stdout_tail: vec![
                "[ERR_PNPM_FETCH_404] GET https://registry.npmjs.org/x: Not Found - 404".to_string(),
            ],
            stderr_tail: vec![
                r"dsh: pnpm failed in profile directory C:\Users\x\.dsh\profiles\web".to_string(),
            ],
            ..Default::default()
        };
        let text = describe_pnpm_failure(&failure);
        assert!(text.contains("ERR_PNPM_FETCH_404"), "{text}");
        assert!(!text.contains("dsh: pnpm failed"), "应过滤 dsh 噪声行：{text}");
    }

    /// BUG-1：未识别的失败仍应带上 stderr（不丢信息），且不编造提示。
    #[test]
    fn test_describe_pnpm_failure_passthrough() {
        let failure = stream::StreamFailure {
            exit_code: Some(2),
            stdout_tail: vec!["some unknown pnpm error".to_string()],
            ..Default::default()
        };
        let text = describe_pnpm_failure(&failure);
        assert!(text.contains("命令退出码 2"), "{text}");
        assert!(text.contains("some unknown pnpm error"), "{text}");
        assert!(!text.contains("提示："), "未识别时不应编造提示：{text}");
    }

    fn fixture_sections() -> Vec<DumpSection> {
        let text = "# == dsh-cost-meter\n- id: cost-meter\n  name: dsh-cost-meter\n# == dshmarket\n- id: dsh-market\n  name: dshmarket\n  disabled: true\n";
        dump::parse_dump(text).unwrap()
    }

    fn manifest_with(deps: &[&str], bundles: &[&str]) -> profile::ProfileManifest {
        profile::ProfileManifest {
            dependencies: deps
                .iter()
                .map(|name| (name.to_string(), "^1.0.0".to_string()))
                .collect(),
            bundles: bundles.iter().map(|name| name.to_string()).collect(),
            patch_reload: Some("live".to_string()),
        }
    }

    #[test]
    fn test_build_views_states() {
        let dir = std::env::temp_dir().join(format!("dsh-launcher-views-{}", std::process::id()));
        let sections = fixture_sections();
        let manifest = manifest_with(
            &["dsh-cost-meter", "dshmarket", "plain-lib"],
            &["dsh-cost-meter", "dshmarket"],
        );
        let mut registry = Registry::empty("web");
        let mut record = PluginRecord::new("dshmarket");
        record.origin = Origin::InHouse;
        record.desired = Some("disabled".to_string());
        record.rows = vec!["dsh-market".to_string()];
        registry.upsert(record);
        let views = build_views(&dir, &manifest, &sections, &registry);
        assert_eq!(views.len(), 3);
        let by_name: BTreeMap<_, _> = views.iter().map(|v| (v.package.as_str(), v)).collect();

        // 临时目录里没有 node_modules → 三个包都判定为 plain（不声明 dsh.bundle）
        for name in ["dsh-cost-meter", "dshmarket", "plain-lib"] {
            assert_eq!(by_name[name].state, PluginState::Plain, "{name}");
        }
        // 行视图仍来自 dump（与包 manifest 无关）
        assert_eq!(by_name["dshmarket"].rows.len(), 1);
        assert_eq!(by_name["dshmarket"].rows[0].id, "dsh-market");
        assert_eq!(by_name["dshmarket"].rows[0].state, RowState::Disabled);
        // 注册表元数据被带出
        assert_eq!(by_name["dshmarket"].origin, Origin::InHouse);
        assert_eq!(by_name["dshmarket"].desired.as_deref(), Some("disabled"));
        // 未登记的包按 spec 分类兜底为 upstream
        assert_eq!(by_name["dsh-cost-meter"].origin, Origin::Upstream);
        assert!(!by_name["dsh-cost-meter"].protected);
    }

    #[test]
    fn test_derive_and_validate_matrix() {
        // 声明 bundle + 在 bundles 中 + 全禁用 → disabled
        assert_eq!(
            state::derive_state(true, true, true, &[RowState::Disabled]),
            PluginState::Disabled
        );
        // 混合 → enabled
        assert_eq!(
            state::derive_state(true, true, true, &[RowState::Disabled, RowState::Enabled]),
            PluginState::Enabled
        );
        // 未在 bundles 中（待对账）→ enabled
        assert_eq!(
            state::derive_state(true, false, true, &[RowState::Disabled]),
            PluginState::Enabled
        );
    }

    #[test]
    fn test_protected_package_flag() {
        let dir = std::env::temp_dir().join(format!("dsh-launcher-prot-{}", std::process::id()));
        let manifest = manifest_with(
            &["@deepseek-ai/dsh-base"],
            &["@deepseek-ai/dsh-base"],
        );
        let views = build_views(&dir, &manifest, &[], &Registry::empty("web"));
        assert!(views[0].protected);
        assert!(PROTECTED_PACKAGES.contains(&"@deepseek-ai/dsh-web-app"));
    }

    #[test]
    fn test_anchor_spec_relative() {
        // 相对路径被锚定为绝对路径（前缀保留）
        let anchored = anchor_spec("link:../plugin");
        assert!(anchored.starts_with("link:"));
        assert!(!anchored.contains("..\\plugin") || anchored.len() > "link:../plugin".len());
        // 非路径 spec 原样返回
        assert_eq!(anchor_spec("dshmarket@^1.0.0"), "dshmarket@^1.0.0");
        assert_eq!(
            anchor_spec("github:o/r#abc1234"),
            "github:o/r#abc1234"
        );
    }

    #[test]
    fn test_op_guard_busy_semantics() {
        // 同键并发 → Busy
        let first = OpGuard::acquire("pkg-a").unwrap();
        let err = OpGuard::acquire("pkg-a").unwrap_err();
        assert_eq!(err.kind, PluginErrorKind::Busy);
        // 不同键可并行
        let other = OpGuard::acquire("pkg-b").unwrap();
        drop(other);
        // 同步进行中禁止普通操作
        drop(first);
        let sync = OpGuard::acquire(SYNC_KEY).unwrap();
        let err = OpGuard::acquire("pkg-c").unwrap_err();
        assert_eq!(err.kind, PluginErrorKind::Busy);
        // 普通操作进行中禁止同步
        drop(sync);
        let op = OpGuard::acquire("pkg-d").unwrap();
        let err = OpGuard::acquire(SYNC_KEY).unwrap_err();
        assert_eq!(err.kind, PluginErrorKind::Busy);
        drop(op);
        // 释放后可再次获取
        assert!(OpGuard::acquire("pkg-a").is_ok());
    }

    /// v0.9.8：损坏 patch 文件识别 + 自愈（样本取自 2026-09-16 实测损坏文件）。
    #[test]
    fn test_inspect_and_heal_corrupt_patch() {
        let dir = std::env::temp_dir().join(format!("dsh-heal-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cordis.patch.yml");
        // 实测损坏样本：模板头 + mnemon + 空区块 + 原内容[33:] 拼接（字节级重构已验证）
        let template = "# Your patch layer for this dsh profile, applied after every bundle layer:
# a top-level YAML array of loader patch entries (id-targeted config
# overrides, disables, and insert lists; `!!js` expressions allowed).
# []
- id: mnemon
  disabled: false
";
        let mark_b = "# >>> dsh-launcher managed v1 — 由启动器维护，请勿手工编辑 >>>
";
        let mark_e = "# <<< dsh-launcher managed v1 <<<
";
        let corrupt = format!("{template}{mark_b}{mark_e}{}", &template[33..]);
        std::fs::write(&path, &corrupt).unwrap();

        // 1) 识别：重复模板头（外部拼接损坏）
        let reason = plugin_inspect(&path).expect("应识别为损坏");
        assert!(reason.contains("模板头重复") || reason.contains("顶层不是"), "reason={reason}");

        // 2) 自愈：文件恢复为合法形态，且原文件已备份
        let backups_before = std::fs::read_dir(&dir).unwrap().count();
        let msg = plugin_heal(&path).expect("自愈应成功");
        assert!(msg.contains("已修复"), "msg={msg}");
        let after = std::fs::read_to_string(&path).unwrap();
        // 健康模板本身含 "profile, applied" 子串，故用「模板头出现次数」判重：
        // 损坏文件 = 两份拼接（模板头 ×2），自愈后应恰为 1 份且不含重复的 mnemon 行。
        assert_eq!(
            after.matches("# Your patch layer for this dsh profile").count(),
            1,
            "自愈后模板头应恰有 1 份，实际:\n{after}"
        );
        assert_eq!(
            after.matches("- id: mnemon").count(),
            1,
            "自愈后不应有重复的 mnemon 行，实际:\n{after}"
        );
        assert!(plugin_inspect(&path).is_none(), "自愈后结构应正常");
        let backups_after = std::fs::read_dir(&dir).unwrap().count();
        assert!(backups_after > backups_before, "应生成备份文件");

        // 3) 结构正常文件不误报
        let ok = format!("{template}");
        std::fs::write(&path, &ok).unwrap();
        assert!(plugin_inspect(&path).is_none(), "正常文件不应报损坏");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn plugin_inspect(path: &std::path::Path) -> Option<String> {
        super::inspect_patch_file(path)
    }
    fn plugin_heal(path: &std::path::Path) -> Result<String, String> {
        super::heal_patch_file(path)
    }

    /// v0.9.7 影子恢复行判定（2026-09-16 故障的回归测试）：
    /// 插件 X（archive-manager）patch 禁用官方 web-app 的 workspace 行并 insert 子类，
    /// disable X 时必须把 workspace 识别为「被 X 替换的官方行」。
    fn section(owner: &str, patched_by: &[&str], rows: Vec<(&str, Option<bool>)>) -> dump::DumpSection {
        dump::DumpSection {
            owner: owner.to_string(),
            patched_by: patched_by.iter().map(|s| s.to_string()).collect(),
            rows: rows
                .into_iter()
                .map(|(id, disabled)| dump::DumpRow {
                    id: id.to_string(),
                    name: None,
                    disabled: disabled.map(dump::DisabledValue::Bool),
                })
                .collect(),
        }
    }

    #[test]
    fn test_shadow_restore_rows_replacement_pattern() {
        let discovered = Discovered {
            profile_dir: std::path::PathBuf::from("C:/tmp/profile"),
            manifest: profile::ProfileManifest::default(),
            sections: vec![
                // web-app 拥有 workspace 行；被 archive-manager patch 禁用
                section(
                    "@deepseek-ai/dsh-web-app",
                    &["@michengai/dsh-archive-manager"],
                    vec![
                        ("session-reference", Some(false)),
                        ("workspace", Some(true)), // 被插件禁用的官方行
                    ],
                ),
                // 插件自己的段：子类行
                section(
                    "@michengai/dsh-archive-manager",
                    &[],
                    vec![
                        ("workspace-archive-manager", Some(true)),
                        ("ui-workspace-archive-manager", Some(true)),
                    ],
                ),
                // 用户 patch 层（绝对路径）：同有禁用行，但不属于官方 bundle 层
                section(
                    "C:\\Users\\t\\.dsh\\profiles\\web\\cordis.patch.yml",
                    &[],
                    vec![("mcp-github", Some(true))],
                ),
                // 另一官方 bundle 的禁用行：段头无 archive-manager 参与 → 不恢复
                section(
                    "@deepseek-ai/dsh-base",
                    &["@deepseek-ai/dsh-web-app"],
                    vec![("telemetry", Some(true))],
                ),
            ],
            degraded_reason: None,
        };
        let rows = shadow_restore_rows(&discovered, "@michengai/dsh-archive-manager");
        assert_eq!(rows, vec!["workspace".to_string()]);
    }

    #[test]
    fn test_shadow_restore_rows_negative_cases() {
        // 无子类行（own_rows 空）→ 无影子行
        let mut d = Discovered {
            profile_dir: std::path::PathBuf::from("C:/tmp/profile"),
            manifest: profile::ProfileManifest::default(),
            sections: vec![section(
                "@deepseek-ai/dsh-web-app",
                &["@some-plugin"],
                vec![("workspace", Some(true))],
            )],
            degraded_reason: None,
        };
        assert!(shadow_restore_rows(&d, "@some-plugin").is_empty());

        // 插件对官方行的禁用属于自己行的改动（id 在 own_rows）→ 不恢复
        d.sections = vec![section(
            "@deepseek-ai/dsh-web-app",
            &["@some-plugin"],
            vec![("plugin-row", Some(true))],
        )];
        // own_rows 需要来自同 sections 的 rows_of_owner，插件段必须有行
        d.sections.push(section("@some-plugin", &[], vec![("plugin-row", Some(true))]));
        assert!(shadow_restore_rows(&d, "@some-plugin").is_empty());

        // 官方行是启用状态 → 不需要恢复
        d.sections = vec![
            section("@deepseek-ai/dsh-web-app", &["@some-plugin"], vec![("workspace", Some(false))]),
            section("@some-plugin", &[], vec![("sub", Some(true))]),
        ];
        assert!(shadow_restore_rows(&d, "@some-plugin").is_empty());
    }
}
