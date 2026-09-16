//! 受管插件注册表（`%APPDATA%\dsh-launcher\plugins.json`）
//!
//! 注册表是**缓存**：磁盘（profile package.json / bundles / dump）是事实源。
//! 注册表只保存无法从磁盘推断的元数据：来源分类、git 仓库与 commit、期望态、
//! 上次同步结果、隔离标记。schemaVersion 参与迁移。

use crate::core::plugin::spec::{Origin, SpecKind};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 当前注册表 schema 版本
pub const SCHEMA_VERSION: u32 = 1;

/// 插件来源描述
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginSource {
    pub kind: SpecKind,
    /// 原始 spec（安装时写入 package.json 的形态）
    pub spec: String,
    /// git 仓库地址（`git ls-remote` 用）
    #[serde(default)]
    pub repo: Option<String>,
    /// git 引用（分支/tag/sha）
    #[serde(default)]
    pub reference: Option<String>,
    /// 已安装的 commit（git 源）
    #[serde(default)]
    pub commit: Option<String>,
}

/// 一次同步的结果
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncRecord {
    /// 同步时间（Unix 秒字符串，避免引入时间库）
    pub at: String,
    /// 同步前版本/commit
    pub from: String,
    /// 同步后版本/commit
    pub to: String,
    /// `ok` / `failed` / `skipped`
    pub result: String,
}

/// 单个插件的注册记录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRecord {
    /// 包名（profile 依赖键）
    pub package: String,
    #[serde(default)]
    pub origin: Origin,
    #[serde(default)]
    pub source: Option<PluginSource>,
    /// 该包贡献的行 id
    #[serde(default)]
    pub rows: Vec<String>,
    /// 期望态（`enabled` / `disabled`）；None = 跟随 bundle 默认
    #[serde(default)]
    pub desired: Option<String>,
    #[serde(default)]
    pub last_sync: Option<SyncRecord>,
    /// 受保护（不可卸载）：dsh-base / dsh-web-app 等模板层
    #[serde(default)]
    pub protected: bool,
    /// 启动失败后被隔离（禁用其行）
    #[serde(default)]
    pub quarantine: bool,
    #[serde(default)]
    pub last_error: Option<String>,
    /// 影子恢复行（v0.9.7）：禁用该插件时被受管启用行恢复的官方行 id。
    /// 重新启用时按此清单移除这些启用行（插件 patch 重新接管），并清空本字段。
    #[serde(default)]
    pub shadow_restored: Vec<String>,
}

impl PluginRecord {
    pub fn new(package: impl Into<String>) -> Self {
        Self {
            package: package.into(),
            origin: Origin::Unknown,
            source: None,
            rows: Vec::new(),
            desired: None,
            last_sync: None,
            protected: false,
            quarantine: false,
            last_error: None,
            shadow_restored: Vec::new(),
        }
    }
}

/// 注册表根
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registry {
    pub schema_version: u32,
    /// 注册表所属 profile
    pub profile: String,
    #[serde(default)]
    pub plugins: Vec<PluginRecord>,
}

impl Registry {
    pub fn empty(profile: &str) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            profile: profile.to_string(),
            plugins: Vec::new(),
        }
    }

    /// 注册表文件路径（与 config.json 同目录）。
    pub fn path() -> PathBuf {
        crate::core::config::AppConfig::config_path()
            .parent()
            .map(|dir| dir.join("plugins.json"))
            .unwrap_or_else(|| PathBuf::from("plugins.json"))
    }

    /// 读取注册表；不存在或损坏时返回空注册表（不阻塞功能，磁盘仍是事实源）。
    pub fn load(profile: &str) -> Self {
        Self::load_from(&Self::path(), profile)
    }

    /// 从指定路径读取（测试注入用）。
    pub fn load_from(path: &Path, profile: &str) -> Self {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::empty(profile);
        };
        match serde_json::from_str::<Registry>(&raw) {
            Ok(mut registry) => {
                if registry.schema_version == 0 {
                    registry.schema_version = SCHEMA_VERSION;
                }
                if registry.profile.is_empty() {
                    registry.profile = profile.to_string();
                }
                registry
            }
            Err(_) => Self::empty(profile),
        }
    }

    /// 原子写注册表。
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path())
    }

    /// 原子写到指定路径（测试注入用）。
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("写入 {} 失败: {e}", tmp.display()))?;
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("替换 {} 失败: {e}", path.display())
        })
    }

    /// 按包名查找
    pub fn find(&self, package: &str) -> Option<&PluginRecord> {
        self.plugins.iter().find(|item| item.package == package)
    }

    /// 插入或整体替换某包记录
    pub fn upsert(&mut self, record: PluginRecord) {
        match self.plugins.iter_mut().find(|item| item.package == record.package) {
            Some(existing) => *existing = record,
            None => self.plugins.push(record),
        }
    }

    /// 按包名删除记录
    pub fn remove(&mut self, package: &str) -> bool {
        let before = self.plugins.len();
        self.plugins.retain(|item| item.package != package);
        before != self.plugins.len()
    }

    /// 设置期望态
    pub fn set_desired(&mut self, package: &str, desired: Option<&str>) {
        if let Some(record) = self.plugins.iter_mut().find(|item| item.package == package) {
            record.desired = desired.map(|s| s.to_string());
        }
    }

    /// 记录错误
    pub fn set_error(&mut self, package: &str, message: Option<String>) {
        if let Some(record) = self.plugins.iter_mut().find(|item| item.package == package) {
            record.last_error = message;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dsh-launcher-registry-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("plugins.json")
    }

    #[test]
    fn test_save_load_roundtrip() {
        let path = temp_path("roundtrip");
        let mut registry = Registry::empty("web");
        let mut record = PluginRecord::new("dsh-cost-meter");
        record.origin = Origin::Upstream;
        record.rows = vec!["cost-meter".to_string()];
        record.desired = Some("enabled".to_string());
        record.source = Some(PluginSource {
            kind: SpecKind::Npm,
            spec: "dsh-cost-meter@^1.7.13".to_string(),
            repo: None,
            reference: None,
            commit: None,
        });
        registry.upsert(record);
        registry.save_to(&path).unwrap();

        let loaded = Registry::load_from(&path, "web");
        assert_eq!(loaded.schema_version, SCHEMA_VERSION);
        assert_eq!(loaded.profile, "web");
        assert_eq!(loaded.plugins.len(), 1);
        assert_eq!(loaded.plugins[0].origin, Origin::Upstream);
        assert_eq!(loaded.plugins[0].rows, vec!["cost-meter"]);
        assert_eq!(
            loaded.plugins[0].source.as_ref().map(|s| s.kind),
            Some(SpecKind::Npm)
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_missing_or_corrupt_returns_empty() {
        let path = temp_path("corrupt");
        assert!(Registry::load_from(&path, "web").plugins.is_empty());
        std::fs::write(&path, "{not json").unwrap();
        assert!(Registry::load_from(&path, "web").plugins.is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_upsert_remove_set() {
        let mut registry = Registry::empty("web");
        registry.upsert(PluginRecord::new("a"));
        registry.upsert(PluginRecord::new("b"));
        registry.upsert(PluginRecord::new("a"));
        assert_eq!(registry.plugins.len(), 2);
        registry.set_desired("a", Some("disabled"));
        assert_eq!(registry.find("a").unwrap().desired.as_deref(), Some("disabled"));
        assert!(registry.remove("a"));
        assert!(!registry.remove("a"));
        assert_eq!(registry.plugins.len(), 1);
    }
}
