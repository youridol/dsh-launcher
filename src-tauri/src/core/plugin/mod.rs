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
            logger.warn(&format!(
                "解析 dsh --dump-config 失败，插件列表降级为只读：{error}"
            ));
            (Vec::new(), Some(error.message))
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
    let desired_entries = managed::upsert(existing.clone(), &updates, &[]);
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
        }
        registry.set_desired(package, Some(if enabled { "enabled" } else { "disabled" }));
        let _ = registry.save();
        return Ok(OpResult::unchanged(
            format!("{package} 已处于{}状态（无变更）", if enabled { "启用" } else { "禁用" }),
            Some(package.to_string()),
        ));
    }

    // 写后校验：重新 dump 并核对目标行
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
}
