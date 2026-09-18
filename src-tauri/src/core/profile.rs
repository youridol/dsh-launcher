//! 官方 API 适配器：**唯一**允许调用 `dsh`/`pnpm` 与读写 profile 文件的地方
//!
//! 边界（ADR-0005 API 一节）：
//! - 行发现：`dsh --profile <p> --dump-config`（只读、不启动插件）；
//! - 依赖变更：`dsh plugin --profile <p> add|remove|update|install ...`；
//! - profile manifest / patch 层：只读 manifest，只写 `cordis.patch.yml` 的受管区块。
//!
//! 其余模块禁止自行 `Command::new("dsh")` 或直接写 `$DSH_HOME`。

use crate::core::dshhome;
use crate::core::github::{self, DshProbe};
use crate::core::plugin::state::{PluginError, PluginErrorKind};
use crate::core::text;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

/// dump / 只读查询的超时
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(120);
/// 依赖变更（pnpm 网络操作）的超时
pub const MUTATION_TIMEOUT: Duration = Duration::from_secs(900);

/// profile manifest 中与插件管理相关的切片
#[derive(Debug, Clone, Default)]
pub struct ProfileManifest {
    /// `dependencies`
    pub dependencies: BTreeMap<String, String>,
    /// `dsh.profile.bundles`
    pub bundles: Vec<String>,
    /// `dsh.profile.patchReload`
    pub patch_reload: Option<String>,
}

/// 解析出的 dsh 可执行入口
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DshEntry {
    /// GitHub 通道：源码目录 + node 直接启动
    SourceDir { dir: PathBuf, node: PathBuf },
    /// PATH 中的 dsh（npm 全局包或可用的自家 shim）。
    ///
    /// `shim` 是 `resolve_dsh()` 解析出的**绝对路径**——必须用它执行，
    /// 不能用 `cmd /C dsh`：PATH 首位可能是指向已删除目录的陈旧 GitHub shim
    /// （见 core/github.rs `resolve_dsh` 注释），`cmd /C dsh` 会命中它并报
    /// 「系统找不到指定的路径」（退出码 1），而真正的 npm shim 排在后面。
    Path { shim: PathBuf },
}

/// GitHub 通道安装目录（存在源码入口时才算可用）。
pub fn install_dir() -> Option<PathBuf> {
    let dir = github::github_clone_dir();
    if dir.join("apps/cli/src/bin.ts").exists() {
        Some(dir)
    } else {
        None
    }
}

/// 解析 node.exe（用户级工具链优先，其次 PATH）。
pub fn node_exe() -> Option<PathBuf> {
    let candidate = crate::core::toolchain::node_dir().join("node.exe");
    if candidate.exists() {
        return Some(candidate);
    }
    let mut probe = crate::core::command::hidden("where");
    probe.arg("node");
    if let Ok(out) = probe.output() {
        if out.status.success() {
            let decoded = text::decode(&out.stdout);
            if let Some(line) = decoded.lines().map(str::trim).find(|l| !l.is_empty()) {
                let path = PathBuf::from(line);
                if path.exists() {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// 解析 dsh 入口；不可安全执行时返回 `DshNotInstalled`。
///
/// 复用 `github::resolve_dsh()` 的静态判定：自家 shim 指向损坏目录时
/// **绝不执行** dsh（否则 pnpm 递归进程爆炸，见 core/github.rs 的说明）。
pub fn resolve_entry() -> Result<DshEntry, PluginError> {
    // 1. GitHub 通道源码目录优先：直接 node 启动 bin.ts（进程树浅、token stdout
    //    实时、taskkill 干净），且完全绕开 PATH 中的任何 dsh.cmd。
    if let Some(dir) = install_dir() {
        let node = node_exe().ok_or_else(|| {
            PluginError::new(
                PluginErrorKind::DshNotInstalled,
                "未找到 node.exe，无法调用 dsh（请先安装 Node 工具链）",
            )
        })?;
        return Ok(DshEntry::SourceDir { dir, node });
    }
    // 2. 无源码目录时看 PATH：`resolve_dsh()` 一次扫描即得「类型 + 可用 shim 绝对路径」
    //    （已跳过损坏的自家 shim、取第一个可安全执行的）。
    let resolved = github::resolve_dsh();
    if let Some(shim) = resolved.shim_path {
        return Ok(DshEntry::Path { shim });
    }
    // 3. 无任何可用入口：区分「只有损坏的自家 shim」（保留防 pnpm 递归爆炸的具名提示）
    //    与「PATH 无 dsh」。
    if resolved.kind == DshProbe::OwnedShimBroken {
        return Err(PluginError::new(
            PluginErrorKind::DshNotInstalled,
            "dsh 安装目录缺失（GitHub shim 指向的目录已不存在或为空），请重新安装 dsh",
        ));
    }
    Err(PluginError::new(
        PluginErrorKind::DshNotInstalled,
        "未找到 dsh（PATH 无 dsh 且无 GitHub 安装目录），请先在版本管理中安装 dsh",
    ))
}

/// 构造 `dsh <args...>` 命令（不 spawn）。
pub fn build_dsh_command(args: &[String]) -> Result<Command, PluginError> {
    match resolve_entry()? {
        DshEntry::SourceDir { dir, node } => {
            let mut cmd = crate::core::command::hidden(&node);
            cmd.current_dir(&dir);
            cmd.args(["--import", "tsx/esm", "apps/cli/src/bin.ts"]);
            cmd.args(args);
            Ok(cmd)
        }
        DshEntry::Path { shim } => {
            // 用解析到的 shim **绝对路径**执行（不能用 `cmd /C dsh`：PATH 首位
            // 可能是陈旧损坏 shim，会遮蔽 npm shim，见 DshEntry::Path 注释）。
            let mut cmd = crate::core::command::hidden_cmd(&shim);
            cmd.args(args);
            Ok(cmd)
        }
    }
}

/// 带超时执行 dsh 并捕获输出。
pub fn run_dsh(args: &[String], timeout: Duration) -> Result<Output, PluginError> {
    let cmd = build_dsh_command(args)?;
    crate::core::command::run_with_timeout(cmd, timeout).map_err(|e| {
        PluginError::internal(format!("执行 dsh 失败: {e}"))
    })
}

/// 执行 `dsh --profile <p> --dump-config` 并返回 stdout。
pub fn dump_config(profile: &str) -> Result<String, PluginError> {
    let args = vec![
        "--profile".to_string(),
        profile.to_string(),
        "--dump-config".to_string(),
    ];
    let out = run_dsh(&args, QUERY_TIMEOUT)?;
    if !out.status.success() {
        return Err(PluginError::internal(format!(
            "dsh --dump-config 失败（退出码 {}）：{}",
            out.status.code().unwrap_or(-1),
            text::decode(&out.stderr).trim()
        )));
    }
    Ok(text::decode(&out.stdout))
}

/// 读取 profile manifest。
pub fn read_manifest(dir: &Path) -> Result<ProfileManifest, PluginError> {
    let path = dir.join("package.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| PluginError::internal(format!("读取 {} 失败: {e}", path.display())))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| PluginError::internal(format!("解析 {} 失败: {e}", path.display())))?;

    let mut dependencies = BTreeMap::new();
    if let Some(map) = value.get("dependencies").and_then(|v| v.as_object()) {
        for (key, item) in map {
            if let Some(spec) = item.as_str() {
                dependencies.insert(key.clone(), spec.to_string());
            }
        }
    }
    let dsh = value.get("dsh");
    let bundles = dsh
        .and_then(|v| v.get("profile"))
        .and_then(|v| v.get("bundles"))
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let patch_reload = dsh
        .and_then(|v| v.get("profile"))
        .and_then(|v| v.get("patchReload"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(ProfileManifest {
        dependencies,
        bundles,
        patch_reload,
    })
}

/// 解析已安装包目录：先 profile `node_modules`，再 dsh 安装目录。
pub fn resolve_package_dir(package: &str, profile_dir: &Path) -> Option<PathBuf> {
    let mut anchors: Vec<PathBuf> = Vec::new();
    anchors.push(profile_dir.join("node_modules").join(package));
    if let Some(dir) = install_dir() {
        anchors.push(dir.join("node_modules").join(package));
    }
    if let Ok(profile_anchor) = std::env::current_dir() {
        anchors.push(profile_anchor.join("node_modules").join(package));
    }
    anchors.into_iter().find(|path| path.join("package.json").exists())
}

/// 读取已安装包的 manifest（找不到返回 None）。
pub fn read_package_manifest(package: &str, profile_dir: &Path) -> Option<serde_json::Value> {
    let dir = resolve_package_dir(package, profile_dir)?;
    let raw = std::fs::read_to_string(dir.join("package.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 包是否声明 `dsh.bundle.patch`（bundle 判定，对齐 plugin.ts:36-45）。
pub fn declares_bundle(package: &str, profile_dir: &Path) -> bool {
    read_package_manifest(package, profile_dir)
        .and_then(|value| {
            value
                .get("dsh")
                .and_then(|v| v.get("bundle"))
                .and_then(|v| v.get("patch"))
                .map(|_| ())
        })
        .is_some()
}

/// 读取已安装包版本。
pub fn package_version(package: &str, profile_dir: &Path) -> Option<String> {
    read_package_manifest(package, profile_dir)
        .and_then(|value| value.get("version").and_then(|v| v.as_str()).map(|s| s.to_string()))
}

/// 读取包名清单（profile 依赖）。
pub fn dependency_names(dir: &Path) -> Result<Vec<String>, PluginError> {
    Ok(read_manifest(dir)?.dependencies.into_keys().collect())
}

/// 备份若干文件到 `dest` 目录（不存在或读取失败的文件跳过），返回备份清单。
///
/// `files` 为 `(绝对路径, 备份子路径)`。**子路径参与落点**：早先按 `file_name()`
/// 落盘，而 `package.json` / `pnpm-lock.yaml` / `pnpm-workspace.yaml` 在同一
/// profile 目录下**同名不同义**，导致三个备份互相覆盖 —— 回滚会写入**另一个文件
/// 的备份内容**。用子路径隔离后每个目标各自独立（ADR-0006 P6）。
pub fn backup_files(
    files: &[(PathBuf, PathBuf)],
    dest: &Path,
) -> Result<Vec<PathBuf>, PluginError> {
    std::fs::create_dir_all(dest)
        .map_err(|e| PluginError::internal(format!("创建备份目录 {} 失败: {e}", dest.display())))?;
    let mut saved = Vec::new();
    for (file, sub_path) in files {
        if !file.exists() {
            continue;
        }
        let target = dest.join(sub_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                PluginError::internal(format!("创建备份目录 {} 失败: {e}", parent.display()))
            })?;
        }
        std::fs::copy(file, &target).map_err(|e| {
            PluginError::internal(format!(
                "备份 {} → {} 失败: {e}",
                file.display(),
                target.display()
            ))
        })?;
        saved.push(target);
    }
    Ok(saved)
}

/// 用备份覆盖回原文件（按**备份子路径**匹配，绝不按文件名匹配）。
pub fn restore_files(
    backup_dir: &Path,
    targets: &[(PathBuf, PathBuf)],
) -> Result<Vec<String>, PluginError> {
    let mut restored = Vec::new();
    for (target, sub_path) in targets {
        let source = backup_dir.join(sub_path);
        if !source.exists() {
            continue;
        }
        std::fs::copy(&source, target).map_err(|e| {
            PluginError::internal(format!(
                "回滚 {} 失败: {e}",
                target.display()
            ))
        })?;
        restored.push(target.display().to_string());
    }
    Ok(restored)
}

/// 时间戳（用于备份目录名，避免引入 chrono 依赖）。
pub fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{now}")
}

/// profile 目录（`$DSH_HOME/profiles/<name>`）。
pub fn profile_dir(profile: &str) -> Result<PathBuf, PluginError> {
    dshhome::profile_dir(profile)
        .map_err(|e| PluginError::internal(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_parsing() {
        let dir = std::env::temp_dir().join(format!("dsh-launcher-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{
              "name": "dsh-profile-web",
              "dependencies": { "dshmarket": "^1.45.0", "dsh-cost-meter": "^1.7.13" },
              "dsh": { "profile": { "bundles": ["@deepseek-ai/dsh-base", "dshmarket"], "patchReload": "live" } }
            }"#,
        )
        .unwrap();
        let manifest = read_manifest(&dir).unwrap();
        assert_eq!(manifest.dependencies.len(), 2);
        assert_eq!(manifest.dependencies.get("dshmarket").map(String::as_str), Some("^1.45.0"));
        assert_eq!(manifest.bundles, vec!["@deepseek-ai/dsh-base", "dshmarket"]);
        assert_eq!(manifest.patch_reload.as_deref(), Some("live"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_backup_and_restore_roundtrip() {
        let root = std::env::temp_dir().join(format!("dsh-launcher-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("profile");
        let bak = root.join("bak");
        std::fs::create_dir_all(&src).unwrap();
        let file = src.join("package.json");
        let targets = vec![(file.clone(), PathBuf::from("package.json"))];
        std::fs::write(&file, "{\"a\":1}").unwrap();
        backup_files(&targets, &bak).unwrap();
        std::fs::write(&file, "{\"a\":2}").unwrap();
        restore_files(&bak, &targets).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"a\":1}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 同目录同名不同义的三个 profile 文件必须各自独立备份（ADR-0006 P6）
    #[test]
    fn test_backup_keeps_same_named_profile_files_separate() {
        let root = std::env::temp_dir().join(format!("dsh-launcher-backup-sep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("profile");
        let bak = root.join("bak");
        std::fs::create_dir_all(&src).unwrap();
        // 三个文件在同一目录，各自内容不同（真实形态）
        let pkg = src.join("package.json");
        let lock = src.join("pnpm-lock.yaml");
        let ws = src.join("pnpm-workspace.yaml");
        std::fs::write(&pkg, "{\"pkg\":1}").unwrap();
        std::fs::write(&lock, "lockfileVersion: 9").unwrap();
        std::fs::write(&ws, "packages: []").unwrap();
        let targets = vec![
            (pkg.clone(), PathBuf::from("package.json")),
            (lock.clone(), PathBuf::from("pnpm-lock.yaml")),
            (ws.clone(), PathBuf::from("pnpm-workspace.yaml")),
        ];
        backup_files(&targets, &bak).unwrap();
        // 破坏全部三个文件
        std::fs::write(&pkg, "{}").unwrap();
        std::fs::write(&lock, "").unwrap();
        std::fs::write(&ws, "").unwrap();
        restore_files(&bak, &targets).unwrap();
        // 各归其位（修复前会因同名碰撞互相覆盖）
        assert_eq!(std::fs::read_to_string(&pkg).unwrap(), "{\"pkg\":1}");
        assert_eq!(
            std::fs::read_to_string(&lock).unwrap(),
            "lockfileVersion: 9"
        );
        assert_eq!(std::fs::read_to_string(&ws).unwrap(), "packages: []");
        let _ = std::fs::remove_dir_all(&root);
    }
}
