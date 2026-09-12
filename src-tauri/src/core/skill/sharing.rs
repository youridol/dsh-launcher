//! 技能/指令共享资源管理（真源 = 官方 `agentsHome` 根）
//!
//! 事实依据（deepseek-harness，逐条标注官方出处）：
//! - 技能根：`<dshHome>/skills`（rank 400）与 `<agentsHome>/skills`（rank 500），
//!   见 `packages/skill/skill-filesystem/src/index.ts:40,253-254`；
//! - `agentsHome` 默认 `$DSH_AGENTS_HOME ?? ~/.agents`，见同文件 `:164`；
//! - 用户全局指令**固定**为 `<dshHome>/AGENTS.md`，见
//!   `packages/context/agent-instructions/src/files.ts:285-291` 与 `render.ts:98,106`；
//! - `CONTEXT.md` **不被 dsh 读取**（全仓无引用），是 agent/技能侧的约定资源。
//!
//! **真源锚定（ADR-0006 D17）**：canonical 就是官方 `agentsHome` 根
//! —— 技能 = `<agentsHome>/skills`（rank 500 **原生覆盖**，**因此技能不需要任何链接**）；
//! 指令 = `<agentsHome>/AGENTS.md`；词表 = `<agentsHome>/CONTEXT.md`。
//! 修复前的 `<agentsHome>/agent` 锚点既非官方扫描根、也已实测失效（三条链接全断链）。
//!
//! 两种共享模式：
//! - **Mode L**：把 `<dshHome>/AGENTS.md` 链接到真源（零 dsh 配置）；
//! - **Mode C**：写 `$DSH_HOME/cordis.patch.yml` 的 shared 受管区块，让 dsh 直接读真源
//!   （文件符号链接无权限时的降级路径）。
//!
//! 冲突处理铁律：**永不删除用户内容**；真实文件/目录一律报冲突并要求显式迁移。

use crate::core::dshhome;
use crate::core::logging::Logger;
use crate::core::plugin::managed;
use crate::core::plugin::state::{PluginError, PluginErrorKind};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 共享资源标识
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShareResource {
    /// 技能根：`<agentsHome>/skills`（rank 500 原生覆盖 → **无需链接**）
    Skills,
    /// 用户全局指令：`<dshHome>/AGENTS.md` ← `<agentsHome>/AGENTS.md`
    AgentsMd,
    /// 词表：`<agentsHome>/CONTEXT.md`（**dsh 不读 → 不建链接**）
    ContextMd,
}

impl ShareResource {
    /// 全部资源
    pub const ALL: [ShareResource; 3] = [
        ShareResource::Skills,
        ShareResource::AgentsMd,
        ShareResource::ContextMd,
    ];

    /// CLI/IPC 使用的键
    pub fn key(&self) -> &'static str {
        match self {
            ShareResource::Skills => "skills",
            ShareResource::AgentsMd => "agents-md",
            ShareResource::ContextMd => "context-md",
        }
    }

    /// 目录名/文件名
    pub fn file_name(&self) -> &'static str {
        match self {
            ShareResource::Skills => dshhome::SKILLS_DIR,
            ShareResource::AgentsMd => dshhome::AGENTS_MD_FILENAME,
            ShareResource::ContextMd => dshhome::CONTEXT_MD_FILENAME,
        }
    }

    /// 是否为目录资源
    pub fn is_dir(&self) -> bool {
        matches!(self, ShareResource::Skills)
    }

    /// 是否需要（也才允许）建立 `<dshHome>` 侧链接。
    ///
    /// - `Skills`：**不需要** —— 官方 rank 500 原生扫 `<agentsHome>/skills`；
    /// - `AgentsMd`：**需要** —— 官方固定读 `<dshHome>/AGENTS.md`；
    /// - `ContextMd`：**不需要** —— dsh 不读该文件，建链接只增加侵入面。
    pub fn needs_link(&self) -> bool {
        matches!(self, ShareResource::AgentsMd)
    }

    /// 由键解析
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|item| item.key() == key)
    }
}

/// 资源状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceState {
    /// 两处都不存在
    Missing,
    /// 已链接到共享真源
    Linked,
    /// 通过 home patch 配置生效（Mode C）
    Config,
    /// `<dshHome>` 侧是真实文件/目录（需迁移，绝不删除）
    Conflict,
    /// 链接指向别处或已断裂
    Broken,
    /// **官方原生根命中**：该资源不需要 `<dshHome>` 侧链接，真源已由官方扫描根覆盖
    ///
    /// 目前只有 `Skills`：官方 `skill-filesystem` 的 `user-agents` 根就是
    /// `<agentsHome>/skills`（rank 500），该根存在即已生效（ADR-0006 D17）。
    Native,
}

/// 路径存在**且可解析**（悬空符号链接不算）。
fn resolves(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok() && path.exists()
}

/// 单个资源的状态（CLI / IPC / UI 的唯一形状）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceStatus {
    pub resource: String,
    /// 共享真源路径（官方 `agentsHome` 根下）
    pub canonical: String,
    /// `<dshHome>` 侧路径
    pub view: String,
    pub state: ResourceState,
    pub detail: String,
    /// 该资源是否需要 `<dshHome>` 侧链接（官方判定，见 `ShareResource::needs_link`）
    pub needs_link: bool,
}

/// 技能共享总状态
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillStatus {
    /// 共享真源根 = 官方 `agentsHome` 根
    pub canonical_root: String,
    /// 官方技能根（`user-agents`，rank 500）= `<agentsHome>/skills`
    pub agents_skills_root: String,
    pub dsh_home: String,
    pub agents_home: String,
    /// 当前进程是否具备创建文件符号链接的能力
    pub link_capable: bool,
    /// 生效中的模式
    ///
    /// - `native`：真源已由官方扫描根原生覆盖（技能无需链接）
    /// - `link`：需要链接的资源全部已链接
    /// - `config`：由 home 层 shared 区块提供
    /// - `mixed`：链接与配置并存
    /// - `none`：两者皆无
    pub active_mode: String,
    /// 用户偏好的模式：`auto` / `link` / `config`
    pub preferred_mode: String,
    pub resources: Vec<ResourceStatus>,
    /// 共享真源 `skills/` 下识别到的技能数量
    pub skill_count: usize,
}

/// 应用结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillApplyReport {
    pub mode: String,
    pub changed: bool,
    pub message: String,
    pub resources: Vec<ResourceStatus>,
}

/// 迁移动作
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrateAction {
    pub resource: String,
    pub action: String,
    pub detail: String,
}

/// 迁移报告
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrateReport {
    pub dry_run: bool,
    pub actions: Vec<MigrateAction>,
}

/// 视图状态判定
///
/// 判定链（顺序即优先级）：
/// 1. 视图侧存在**符号链接** → 目标与真源同源且可解析即 `Linked`；
///    否则 `Broken`（含**断链**、指向别处）；
/// 2. 视图侧是真实文件/目录 → `Conflict`（绝不删除）；
/// 3. 该资源**不需要链接**（官方原生根，如 `Skills`）→ 真源在即 `Native`，否则 `Missing`；
/// 4. 否则 `Missing`（需要链接但视图不存在）。
///
/// 注意：第 1 步必须在第 3 步之前 —— 否则"不需要链接"的资源视图上留下的**断链**
/// 会被误报成 `Native`，既掩盖 `~/.dsh` 的断链，也让清理动作失去判定依据。
fn detect(resource: ShareResource) -> ResourceStatus {
    let canonical = canonical_path(resource);
    let view = view_path(resource);
    let (state, detail) = if let Ok(target) = std::fs::read_link(&view) {
        // 视图侧是链接（junction / symlink）：按链接语义判定。
        //
        // G6（审计 RT-05）：此前先 `symlink_metadata().is_ok() && read_link().is_ok()`
        // 再 `read_link().expect(...)` —— 两次调用间存在 TOCTOU（链接被移除即 panic）。
        // 改为一次 `let Ok(target) = read_link(&view)`，彻底消除 panic 路径。
        let resolved = if target.is_absolute() {
            target.clone()
        } else {
            view.parent().map(|p| p.join(&target)).unwrap_or(target.clone())
        };
        if same_path(&resolved, &canonical) {
            if resolves(&canonical) {
                (
                    ResourceState::Linked,
                    format!("已链接到 {}", canonical.display()),
                )
            } else {
                (
                    ResourceState::Broken,
                    format!(
                        "链接指向 {}，但该目标不存在（断链）",
                        canonical.display()
                    ),
                )
            }
        } else {
            (
                ResourceState::Broken,
                format!(
                    "链接指向 {}，期望 {}",
                    resolved.display(),
                    canonical.display()
                ),
            )
        }
    } else if std::fs::symlink_metadata(&view).is_ok() {
        // 真实文件/目录：绝不替换、绝不删除
        (
            ResourceState::Conflict,
            format!(
                "{} 是真实文件/目录（迁移向导可合并，绝不自动删除）",
                view.display()
            ),
        )
    } else if !resource.needs_link() {
        // 官方原生根：视图侧无需链接
        if resolves(&canonical) {
            (
                ResourceState::Native,
                format!(
                    "官方原生根已覆盖（{} 存在，无需 {} 侧链接）",
                    canonical.display(),
                    dsh_home().display()
                ),
            )
        } else {
            (
                ResourceState::Missing,
                format!("{} 不存在（官方原生根未命中）", canonical.display()),
            )
        }
    } else {
        (
            ResourceState::Missing,
            format!("{} 不存在", view.display()),
        )
    };
    ResourceStatus {
        resource: resource.key().to_string(),
        canonical: canonical.display().to_string(),
        view: view.display().to_string(),
        state,
        detail,
        needs_link: resource.needs_link(),
    }
}

/// 用户偏好（`%APPDATA%\dsh-launcher\skills.json`）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSettings {
    #[serde(default = "default_schema")]
    pub schema_version: u32,
    /// `auto` / `link` / `config`
    #[serde(default)]
    pub preferred_mode: Option<String>,
    #[serde(default)]
    pub last_applied: Option<String>,
}

fn default_schema() -> u32 {
    1
}

impl Default for SkillSettings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            preferred_mode: None,
            last_applied: None,
        }
    }
}

impl SkillSettings {
    fn path() -> PathBuf {
        crate::core::config::AppConfig::config_path()
            .parent()
            .map(|dir| dir.join("skills.json"))
            .unwrap_or_else(|| PathBuf::from("skills.json"))
    }

    fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|raw| serde_json::from_str::<SkillSettings>(&raw).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("写入 {} 失败: {e}", tmp.display()))?;
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("替换 {} 失败: {e}", path.display())
        })
    }
}

// ============================ 探测 ============================

/// `<dshHome>` 侧的视图路径。
///
/// - `Skills`：Map 到**官方 dsh-home 技能根** `<dshHome>/skills`（rank 400）。
///   它**不**由启动器建链接（真源 `<agentsHome>/skills` 是 rank 500，原生覆盖）；
///   本路径只用于**探测/展示**用户在该处已有的技能目录（`conflict` 时不动作）。
/// - `AgentsMd`：`<dshHome>/AGENTS.md`（官方固定读该路径 → 链接目标）。
/// - `ContextMd`：`<dshHome>/CONTEXT.md`（仅探测/展示，不建链接）。
pub fn view_path(resource: ShareResource) -> PathBuf {
    dsh_home().join(resource.file_name())
}

/// 共享真源路径（ADR-0006 D17：锚在官方 `agentsHome` 根）。
///
/// - `Skills` → `<agentsHome>/skills`（官方 `user-agents` 根，rank 500）
/// - `AgentsMd` → `<agentsHome>/AGENTS.md`
/// - `ContextMd` → `<agentsHome>/CONTEXT.md`
pub fn canonical_path(resource: ShareResource) -> PathBuf {
    dshhome::shared_agent_dir().join(resource.file_name())
}

fn dsh_home() -> PathBuf {
    dshhome::dsh_home()
}

/// 路径是否同源（优先 canonicalize，失败退化为字符串比较）
fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    let canon = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let a = canon(left);
    let b = canon(right);
    if a == b {
        return true;
    }
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// 统计共享真源下的技能数量（目录 bundle `SKILL.md` 或扁平 `.md`）。
pub fn count_skills(root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            if path.is_dir() {
                path.join("SKILL.md").is_file()
            } else {
                path.extension().map(|ext| ext == "md").unwrap_or(false)
            }
        })
        .count()
}

/// 探测创建文件符号链接的能力（Windows 需要开发者模式/特权）。
pub fn link_capable() -> bool {
    #[cfg(windows)]
    {
        let dir = std::env::temp_dir().join(format!(
            "dsh-launcher-linkprobe-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        if std::fs::create_dir_all(&dir).is_err() {
            return false;
        }
        let target = dir.join("target.txt");
        let link = dir.join("link.txt");
        let ok = std::fs::write(&target, "probe").is_ok()
            && std::os::windows::fs::symlink_file(&target, &link).is_ok();
        let _ = std::fs::remove_dir_all(&dir);
        ok
    }
    #[cfg(not(windows))]
    {
        true
    }
}

// ============================ 链接创建 ============================

/// 创建目录链接（Windows 用 junction，免特权）。
fn create_dir_link(link: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    #[cfg(windows)]
    {
        let mut cmd = crate::core::command::hidden("cmd");
        cmd.args(["/D", "/C", "mklink", "/J"]);
        cmd.arg(link);
        cmd.arg(target);
        let out = cmd
            .output()
            .map_err(|e| format!("执行 mklink 失败: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "创建目录链接失败: {}",
                crate::core::text::decode(&out.stdout).trim()
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link)
            .map_err(|e| format!("创建目录链接失败: {e}"))
    }
}

/// 创建文件链接。
fn create_file_link(link: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
            .map_err(|e| format!("创建文件链接失败（需开发者模式/符号链接特权）: {e}"))
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).map_err(|e| format!("创建文件链接失败: {e}"))
    }
}

/// 删除链接（junction 需按目录删除）。
fn remove_link(path: &Path) -> Result<(), String> {
    if std::fs::remove_file(path).is_ok() {
        return Ok(());
    }
    std::fs::remove_dir(path).map_err(|e| format!("删除链接 {} 失败: {e}", path.display()))
}

/// 确保共享真源存在。
///
/// **只在该资源确实需要启动器参与时**才创建：真源位于官方 `agentsHome` 根下，
/// 其中 `<agentsHome>/skills` 是官方扫描根 —— 若用户还没有该目录，启动器**不**替
/// 用户凭空造一个空技能根（那是 dsh/用户的地盘）；技能侧只需探测与展示。
/// 指令文件（`<agentsHome>/AGENTS.md`）需要被链接，故在缺失时补一个空文件作为
/// 链接目标（真实内容由用户维护）。
fn ensure_canonical(resource: ShareResource) -> Result<(), String> {
    let path = canonical_path(resource);
    if path.exists() {
        return Ok(());
    }
    if resource.is_dir() {
        std::fs::create_dir_all(&path)
            .map_err(|e| format!("创建 {} 失败: {e}", path.display()))?;
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
        }
        std::fs::write(&path, "")
            .map_err(|e| format!("创建 {} 失败: {e}", path.display()))?;
    }
    Ok(())
}

/// 让单个资源生效（必要时建立/修复 `<dshHome>` 侧链接）。
///
/// - **不需要链接**的资源（官方原生根，如 `Skills`）：只探测与展示，**零文件操作**；
/// - **需要链接**的资源（`AgentsMd`）：`Missing`/`Broken` → 建/修链接；
///   `Conflict`（真实文件）→ 拒绝并提示走 `skill migrate`；已 `Linked` → 幂等返回。
/// - **不建链接**的资源（`ContextMd`）：同"不需要链接"，且真源缺失时只报告。
fn link_resource(resource: ShareResource) -> Result<ResourceStatus, PluginError> {
    if !resource.needs_link() {
        // 官方原生根：不创建、不链接、不删除任何东西
        return Ok(detect(resource));
    }
    ensure_canonical(resource).map_err(PluginError::internal)?;
    let status = detect(resource);
    match status.state {
        ResourceState::Linked | ResourceState::Config => Ok(status),
        ResourceState::Conflict => Err(PluginError::new(
            PluginErrorKind::CapabilityMissing,
            format!(
                "{} 是真实文件/目录，不能直接替换；请先执行 skill migrate 合并到共享真源",
                status.view
            ),
        )),
        ResourceState::Native => Ok(status),
        ResourceState::Missing | ResourceState::Broken => {
            if status.state == ResourceState::Broken {
                remove_link(Path::new(&status.view)).map_err(PluginError::internal)?;
            }
            let canonical = canonical_path(resource);
            let view = PathBuf::from(&status.view);
            if resource.is_dir() {
                create_dir_link(&view, &canonical).map_err(PluginError::internal)?;
            } else {
                create_file_link(&view, &canonical).map_err(PluginError::internal)?;
            }
            Ok(detect(resource))
        }
    }
}

/// 该资源是否处于"生效中"（真源被官方根覆盖 / 已链接 / 由配置提供）。
fn is_active(state: ResourceState) -> bool {
    matches!(
        state,
        ResourceState::Native | ResourceState::Linked | ResourceState::Config
    )
}

// ============================ 对外动作 ============================

/// 判断一个**断链**是否由启动器自己创建（目标落在官方 `agentsHome` 根下）。
///
/// 只用于「清理启动器自己留下的断链」这一动作：目标不在 `agentsHome` 下的一律
/// 视为用户自己建的链接，**绝不删除**。
fn broken_link_owned_by_launcher(view: &Path) -> bool {
    let Ok(target) = std::fs::read_link(view) else {
        return false;
    };
    let resolved = if target.is_absolute() {
        target
    } else {
        view.parent().map(|dir| dir.join(&target)).unwrap_or(target)
    };
    resolved.starts_with(dshhome::agents_home())
}

/// 一次性的断链清理 + 链接修复（ADR-0006 D17 / G3 清单 approved）。
///
/// 做三件事，**全部可幂等重跑**：
/// 1. **修复**需要链接的资源（`agents-md`）：断链/缺失 → 重建为指向真源的链接；
/// 2. **清理**不再需要链接、且启动器自己留下的**断链**（`skills`、`context-md`）——
///    它们是 D17 之前的产物，留着会让 `~/.dsh` 永久存在断链（B5 要求"无断链"）；
/// 3. **保留**一切含用户内容的真实文件/目录，以及**指向 `agentsHome` 之外**的链接。
///
/// 返回 `(已修复, 已清理断链, 保留)` 三类动作说明。
pub fn repair_links(_profile_name: &str, logger: &Arc<Logger>) -> Result<MigrateReport, PluginError> {
    let mut actions: Vec<MigrateAction> = Vec::new();
    for resource in ShareResource::ALL {
        let view = view_path(resource);
        let status = detect(resource);

        if resource.needs_link() {
            // ① 修复：Missing / Broken → 重建链接
            if matches!(status.state, ResourceState::Missing | ResourceState::Broken) {
                let before = status.state;
                let after = link_resource(resource)?;
                actions.push(MigrateAction {
                    resource: resource.key().to_string(),
                    action: "repaired".to_string(),
                    detail: format!(
                        "{} {} → {}（目标 {}）",
                        view.display(),
                        format!("{before:?}").to_lowercase(),
                        format!("{:?}", after.state).to_lowercase(),
                        canonical_path(resource).display()
                    ),
                });
            } else {
                actions.push(MigrateAction {
                    resource: resource.key().to_string(),
                    action: "kept".to_string(),
                    detail: format!("{} 已是 {:?}，无需变更", view.display(), status.state),
                });
            }
            continue;
        }

        // ② 不需要链接的资源：只清理**启动器自己创建的断链**
        if status.state == ResourceState::Broken && broken_link_owned_by_launcher(&view) {
            remove_link(&view).map_err(PluginError::internal)?;
            actions.push(MigrateAction {
                resource: resource.key().to_string(),
                action: "cleaned-broken-link".to_string(),
                detail: format!(
                    "已删除启动器创建的断链 {}（{}；D17 后该资源不需要链接，真源 {}）",
                    view.display(),
                    status.detail,
                    canonical_path(resource).display()
                ),
            });
        } else if status.state == ResourceState::Broken {
            // 指向 agentsHome 之外的链接：用户自己建的，保留
            actions.push(MigrateAction {
                resource: resource.key().to_string(),
                action: "kept".to_string(),
                detail: format!(
                    "保留 {}（断链但**非**启动器创建：{}）",
                    view.display(),
                    status.detail
                ),
            });
        } else {
            actions.push(MigrateAction {
                resource: resource.key().to_string(),
                action: "kept".to_string(),
                detail: format!("保留 {}（{:?}）", view.display(), status.state),
            });
        }
    }
    logger.info(&format!(
        "技能/指令链接修复完成：{} 项动作",
        actions.len()
    ));
    Ok(MigrateReport {
        dry_run: false,
        actions,
    })
}

/// 读取当前状态。
pub fn status() -> SkillStatus {
    let settings = SkillSettings::load();
    let resources: Vec<ResourceStatus> = ShareResource::ALL.iter().copied().map(detect).collect();
    let linked = resources
        .iter()
        .filter(|item| item.state == ResourceState::Linked)
        .count();
    let shared_body = managed::read_shared_body(&dshhome::home_patch_path())
        .ok()
        .flatten()
        .filter(|body| !body.trim().is_empty());
    // 生效中的资源：真源已被官方根覆盖（Native）/ 已链接（Linked）/ 由配置提供（Config）
    let active = resources
        .iter()
        .filter(|item| is_active(item.state))
        .count();
    // 「缺失」只算**本应由启动器补上却没补上**的：不需要链接的资源缺失不算欠账
    let outstanding = resources
        .iter()
        .filter(|item| item.needs_link && item.state == ResourceState::Missing)
        .count();
    let has_native = resources
        .iter()
        .any(|item| item.state == ResourceState::Native);
    let active_mode = if shared_body.is_some() && linked > 0 {
        "mixed".to_string()
    } else if shared_body.is_some() {
        "config".to_string()
    } else if linked > 0 && outstanding == 0 && !has_native {
        "link".to_string()
    } else if has_native {
        // 至少一个资源由官方原生扫描根覆盖（技能 rank 500）；
        // 仍有需要链接却缺失的资源时，细节由各自 `ResourceStatus.state = missing` 如实呈现
        "native".to_string()
    } else if linked > 0 {
        "link".to_string()
    } else if active > 0 {
        "native".to_string()
    } else {
        "none".to_string()
    };
    SkillStatus {
        canonical_root: dshhome::shared_agent_dir().display().to_string(),
        agents_skills_root: dshhome::agents_skills_dir().display().to_string(),
        dsh_home: dsh_home().display().to_string(),
        agents_home: dshhome::agents_home().display().to_string(),
        link_capable: link_capable(),
        active_mode,
        preferred_mode: settings
            .preferred_mode
            .unwrap_or_else(|| "auto".to_string()),
        resources,
        skill_count: count_skills(&dshhome::agents_skills_dir()),
    }
}

/// YAML 单引号转义。
fn yaml_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// 构造 Mode C 的 shared 区块体。
///
/// ADR-0006 D17 后的锚点：
/// - 技能：`agentsHome` = 官方 `agentsHome` 根本身 —— `skill-filesystem` 会扫
///   `<agentsHome>/skills`（rank 500），因此**配置即等于官方原生根**，不需要任何链接；
/// - 指令：`dshHome` = 真源根，使 `agent-instructions` 的
///   `<dshHome>/AGENTS.md`（`files.ts:285-291`）解析到 `<agentsHome>/AGENTS.md`。
pub fn shared_config_body() -> String {
    let canonical = dshhome::shared_agent_dir().display().to_string();
    format!(
        "- id: skill-filesystem\n  config:\n    agentsHome: {}\n- id: agent-instructions\n  config:\n    dshHome: {}\n    maxBytes: 65536\n",
        yaml_quote(&canonical),
        yaml_quote(&canonical)
    )
}

/// 校验目标行存在（写 home 层前必须确认，避免无意义告警）。
fn target_rows_exist(profile_name: &str, logger: &Arc<Logger>) -> Result<(), PluginError> {
    let text = crate::core::profile::dump_config(profile_name)?;
    let sections = crate::core::plugin::dump::parse_dump(&text)
        .map_err(|e| PluginError::internal(e))?;
    let index = crate::core::plugin::dump::index_by_id(&sections);
    let missing: Vec<&str> = ["skill-filesystem", "agent-instructions"]
        .into_iter()
        .filter(|id| !index.contains_key(*id))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        logger.warn(&format!(
            "profile {profile_name} 未挂载行 {missing:?}，Mode C 不可用"
        ));
        Err(PluginError::new(
            PluginErrorKind::CapabilityMissing,
            format!(
                "当前 profile 未挂载 {}，无法用配置模式共享技能；请改用链接模式",
                missing.join(", ")
            ),
        ))
    }
}

/// 应用共享模式。
///
/// `mode`：`auto`（探测链接能力后自动选）/ `link` / `config`。
/// `resource`：仅处理单个资源（None = 全部）。
pub fn apply(
    profile_name: &str,
    mode: &str,
    resource: Option<&str>,
    logger: &Arc<Logger>,
) -> Result<SkillApplyReport, PluginError> {
    let selected: Vec<ShareResource> = match resource {
        Some(key) => vec![ShareResource::from_key(key).ok_or_else(|| {
            PluginError::not_found(format!("未知的共享资源: {key}（可选 skills/agents-md/context-md）"))
        })?],
        None => ShareResource::ALL.to_vec(),
    };
    let effective = match mode {
        "auto" => {
            // 真源锚在官方 `agentsHome` 根：技能由 rank 500 原生覆盖，**不需要链接**；
            // 因此只有"确实需要链接的资源"（指令文件）不可链接时才降级到 Mode C。
            let needing = selected.iter().any(|item| item.needs_link());
            if !needing || link_capable() {
                "link"
            } else {
                "config"
            }
        }
        "link" | "config" => mode,
        other => {
            return Err(PluginError::illegal(format!(
                "未知共享模式 {other:?}（可选 auto/link/config）"
            )))
        }
    };

    let mut changed = false;
    let mut statuses: Vec<ResourceStatus> = Vec::new();
    if effective == "link" {
        for item in &selected {
            let before = detect(*item);
            let after = link_resource(*item)?;
            if before.state != after.state {
                changed = true;
                logger.info(&format!(
                    "技能共享：{} {} → {}",
                    item.key(),
                    format!("{:?}", before.state).to_lowercase(),
                    format!("{:?}", after.state).to_lowercase()
                ));
            }
            statuses.push(after);
        }
    } else {
        // Mode C：写 home 层 shared 区块（先确认目标行存在）
        target_rows_exist(profile_name, logger)?;
        let body = shared_config_body();
        let home_patch = dshhome::home_patch_path();
        let existing = managed::read_shared_body(&home_patch)
            .map_err(|e| PluginError::new(PluginErrorKind::ManagedBlockConflict, e))?;
        let outcome = managed::apply_shared_body(&home_patch, Some(&body)).map_err(|e| {
            PluginError::new(PluginErrorKind::ManagedBlockConflict, e)
        })?;
        if outcome == managed::BlockOutcome::Written {
            changed = true;
            logger.info(&format!(
                "技能共享：已写入 {} 的 shared 区块（agentsHome/dshHome → 官方 agentsHome 根）",
                home_patch.display()
            ));
        } else if existing.as_deref() == Some(body.trim()) {
            // 无变化
        }
        for item in &selected {
            let mut item_status = detect(*item);
            // 真源缺失 / 未被官方根覆盖时，说明该资源由 shared 区块提供（而非链接）
            if matches!(
                item_status.state,
                ResourceState::Missing | ResourceState::Native
            ) {
                item_status.state = ResourceState::Config;
                item_status.detail =
                    "由 $DSH_HOME/cordis.patch.yml 的 shared 区块提供（无链接）".to_string();
            }
            statuses.push(item_status);
        }
    }

    let mut settings = SkillSettings::load();
    settings.preferred_mode = Some(mode.to_string());
    settings.last_applied = Some(crate::core::profile::timestamp());
    if let Err(e) = settings.save() {
        logger.warn(&format!("保存技能共享设置失败: {e}"));
    }

    let message = if changed {
        format!("共享模式已应用（{effective}）")
    } else {
        format!("共享模式无需变更（{effective}）")
    };
    Ok(SkillApplyReport {
        mode: effective.to_string(),
        changed,
        message,
        resources: statuses,
    })
}

/// 迁移冲突资源到共享真源（`dry_run` 只报告）。
///
/// 规则：
/// - `skills` 目录：把 `~/.dsh/skills` 下缺失的条目复制进真源（同名冲突保留两份，
///   后者加 `.from-dsh-home` 后缀），随后把原目录改名保留并建立链接；
/// - `AGENTS.md` / `CONTEXT.md`：真源缺失则移动过去；两边都有且内容不同则保留副本
///   `<name>.from-dsh-home`；随后建立链接。
///
/// **任何情况下都不删除用户内容**：原视图一律改名保留（`*.migrated-<ts>`）或移动进真源。
pub fn migrate(dry_run: bool, logger: &Arc<Logger>) -> Result<MigrateReport, PluginError> {
    let mut actions: Vec<MigrateAction> = Vec::new();
    for resource in ShareResource::ALL {
        let status = detect(resource);
        if status.state != ResourceState::Conflict {
            continue;
        }
        let canonical = canonical_path(resource);
        let view = view_path(resource);
        if resource.is_dir() {
            let entries = std::fs::read_dir(&view)
                .map(|iter| iter.flatten().map(|entry| entry.path()).collect::<Vec<_>>())
                .unwrap_or_default();
            let mut copied = 0usize;
            let mut renamed = 0usize;
            for entry in entries {
                let Some(name) = entry.file_name() else { continue };
                let target = canonical.join(&name);
                if target.exists() {
                    renamed += 1;
                    if !dry_run {
                        let alt = canonical.join(format!("{}.from-dsh-home", name.to_string_lossy()));
                        copy_recursive(&entry, &alt).map_err(PluginError::internal)?;
                    }
                } else {
                    copied += 1;
                    if !dry_run {
                        copy_recursive(&entry, &target).map_err(PluginError::internal)?;
                    }
                }
            }
            if !dry_run {
                park_view(&view)?;
            }
            actions.push(MigrateAction {
                resource: resource.key().to_string(),
                action: if dry_run { "plan" } else { "merged" }.to_string(),
                detail: format!(
                    "合并 {copied} 个新条目、{renamed} 个同名冲突（保留为 .from-dsh-home）；原目录改名保留{}",
                    if dry_run { "（预演，未落盘）" } else { "" }
                ),
            });
        } else {
            let view_text = std::fs::read_to_string(&view).unwrap_or_default();
            if !canonical.exists() {
                if !dry_run {
                    if let Some(parent) = canonical.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            PluginError::internal(format!("创建 {} 失败: {e}", parent.display()))
                        })?;
                    }
                    std::fs::rename(&view, &canonical).map_err(|e| {
                        PluginError::internal(format!("迁移 {} 失败: {e}", view.display()))
                    })?;
                }
                actions.push(MigrateAction {
                    resource: resource.key().to_string(),
                    action: if dry_run { "plan" } else { "moved" }.to_string(),
                    detail: format!("移动到 {}", canonical.display()),
                });
            } else {
                let canonical_text = std::fs::read_to_string(&canonical).unwrap_or_default();
                if canonical_text.trim() == view_text.trim() {
                    if !dry_run {
                        park_view(&view)?;
                    }
                    actions.push(MigrateAction {
                        resource: resource.key().to_string(),
                        action: if dry_run { "plan" } else { "kept" }.to_string(),
                        detail: "两边内容一致，原文件改名保留并建立链接".to_string(),
                    });
                } else {
                    if !dry_run {
                        let alt =
                            canonical.join(format!("{}.from-dsh-home", resource.file_name()));
                        std::fs::write(&alt, &view_text).map_err(|e| {
                            PluginError::internal(format!("写入 {} 失败: {e}", alt.display()))
                        })?;
                        park_view(&view)?;
                    }
                    actions.push(MigrateAction {
                        resource: resource.key().to_string(),
                        action: if dry_run { "plan" } else { "kept-both" }.to_string(),
                        detail: format!(
                            "内容不同，保留副本 {}.from-dsh-home，原文件改名保留",
                            resource.file_name()
                        ),
                    });
                }
            }
        }
        // 迁移完成后建立链接（dry_run 跳过）
        if !dry_run {
            let linked = link_resource(resource)?;
            logger.info(&format!(
                "技能共享迁移：{} → {}",
                resource.key(),
                format!("{:?}", linked.state).to_lowercase()
            ));
        }
    }
    if actions.is_empty() {
        actions.push(MigrateAction {
            resource: "all".to_string(),
            action: "noop".to_string(),
            detail: "没有需要迁移的冲突资源".to_string(),
        });
    }
    Ok(MigrateReport { dry_run, actions })
}

/// 把原视图改名保留（`*.migrated-<ts>`），绝不删除。
fn park_view(view: &Path) -> Result<(), PluginError> {
    let parked = view.with_file_name(format!(
        "{}.migrated-{}",
        view.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "view".to_string()),
        crate::core::profile::timestamp()
    ));
    std::fs::rename(view, &parked).map_err(|e| {
        PluginError::internal(format!(
            "保留原视图 {} → {} 失败: {e}",
            view.display(),
            parked.display()
        ))
    })
}

/// 递归复制（目录/文件）。
fn copy_recursive(from: &Path, to: &Path) -> Result<(), String> {
    if from.is_dir() {
        std::fs::create_dir_all(to).map_err(|e| format!("创建 {} 失败: {e}", to.display()))?;
        for entry in std::fs::read_dir(from)
            .map_err(|e| format!("读取 {} 失败: {e}", from.display()))?
            .flatten()
        {
            let child_to = to.join(entry.file_name());
            copy_recursive(&entry.path(), &child_to)?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
        }
        std::fs::copy(from, to).map_err(|e| {
            format!("复制 {} → {} 失败: {e}", from.display(), to.display())
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_keys() {
        assert_eq!(ShareResource::from_key("skills"), Some(ShareResource::Skills));
        assert_eq!(
            ShareResource::from_key("agents-md"),
            Some(ShareResource::AgentsMd)
        );
        assert_eq!(
            ShareResource::from_key("context-md"),
            Some(ShareResource::ContextMd)
        );
        assert_eq!(ShareResource::from_key("nope"), None);
        assert!(ShareResource::Skills.is_dir());
        assert!(!ShareResource::AgentsMd.is_dir());
    }

    #[test]
    fn test_shared_config_body_contains_targets() {
        let body = shared_config_body();
        assert!(body.contains("- id: skill-filesystem"));
        assert!(body.contains("agentsHome:"));
        assert!(body.contains("- id: agent-instructions"));
        assert!(body.contains("dshHome:"));
        assert!(body.contains("maxBytes: 65536"));
    }

    #[test]
    fn test_yaml_quote_escapes_single_quote() {
        assert_eq!(yaml_quote("a'b"), "'a''b'");
    }

    #[test]
    fn test_count_skills() {
        let root = std::env::temp_dir().join(format!("dsh-launcher-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("code-review")).unwrap();
        std::fs::write(root.join("code-review").join("SKILL.md"), "# x").unwrap();
        std::fs::create_dir_all(root.join("not-a-skill")).unwrap();
        std::fs::write(root.join("flat.md"), "# y").unwrap();
        std::fs::write(root.join("ignore.txt"), "z").unwrap();
        assert_eq!(count_skills(&root), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_copy_recursive() {
        let root = std::env::temp_dir().join(format!("dsh-launcher-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let from = root.join("from");
        std::fs::create_dir_all(from.join("sub")).unwrap();
        std::fs::write(from.join("sub").join("a.txt"), "a").unwrap();
        let to = root.join("to");
        copy_recursive(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(to.join("sub").join("a.txt")).unwrap(), "a");
        let _ = std::fs::remove_dir_all(&root);
    }
}
