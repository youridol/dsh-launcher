//! 插件依赖 spec 的分类与解析
//!
//! 分类决定同步策略（ADR-0005 D6）：
//! - `upstream`：registry 版本区间、git 仓库 —— 后台同步任务可自动推进；
//! - `in-house`：`link:`/`file:`/相对或绝对路径/本地 tarball —— **永不被同步任务改动**；
//! - `unknown`：无法判定的形态（例如 http tarball URL）—— 自动同步默认关闭。

use serde::{Deserialize, Serialize};

/// 插件来源类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// 上游插件（可自动同步）
    Upstream,
    /// 自研/本地插件（不自动同步）
    InHouse,
    /// 无法判定（不自动同步，需用户确认）
    #[default]
    Unknown,
}

/// 依赖 spec 的形态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SpecKind {
    /// registry 包名（可带版本区间）
    Npm,
    /// git 仓库（可钉 commit）
    Git,
    /// 本地路径（link:/file:/相对/绝对）
    Path,
    /// 本地 tarball
    Tarball,
    /// 其它
    #[default]
    Unknown,
}

/// 分类结果
pub fn classify(spec: &str) -> (SpecKind, Origin) {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return (SpecKind::Unknown, Origin::Unknown);
    }
    let lower = trimmed.to_ascii_lowercase();

    if lower.starts_with("link:") || lower.starts_with("file:") {
        return (SpecKind::Path, Origin::InHouse);
    }
    if is_git_spec(&lower) {
        return (SpecKind::Git, Origin::Upstream);
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        // BUG-3（审计）：已知 Git 托管站点的裸 URL（无 `.git` 后缀）也是 git 依赖。
        //
        // 实证（pnpm 11.24）：
        //   pnpm add "https://github.com/dsh-market/dsh-market"
        //   → dshmarket github:dsh-market/dsh-market   （成功，被规范化成 git 依赖）
        // 但 `is_git_spec` 此前只认带 `.git`/`.git#` 的形态，于是这类 URL 落到
        // `Unknown` 分支 → **不参与 upstream 自动同步**（`sync.rs` 仅处理 Upstream），
        // 且无法从 spec 推断包名。而 `dsh plugin` 是 pnpm 的薄转发器（官方
        // `apps/cli/src/plugin.ts:120-163`），pnpm 既然能装，就应按 git 源对待。
        if looks_like_hosted_git_url(&lower) {
            return (SpecKind::Git, Origin::Upstream);
        }
        // 其余远端 tarball / 压缩包 URL：形态可执行但无法判定版本推进策略
        return (SpecKind::Unknown, Origin::Unknown);
    }
    if lower.ends_with(".tgz") || lower.ends_with(".tar.gz") {
        return (SpecKind::Tarball, Origin::InHouse);
    }
    if is_relative_or_absolute_path(trimmed) {
        return (SpecKind::Path, Origin::InHouse);
    }
    if lower.starts_with("workspace:") {
        return (SpecKind::Unknown, Origin::InHouse);
    }
    (SpecKind::Npm, Origin::Upstream)
}

/// 是否为相对/绝对本地路径（Windows 盘符或 UNC 也算）。
fn is_relative_or_absolute_path(spec: &str) -> bool {
    if spec.starts_with("./")
        || spec.starts_with("../")
        || spec.starts_with(".\\")
        || spec.starts_with("..\\")
        || spec.starts_with('/')
        || spec.starts_with('\\')
    {
        return true;
    }
    let bytes: Vec<char> = spec.chars().collect();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == ':'
        && (bytes[2] == '\\' || bytes[2] == '/')
    {
        return true;
    }
    false
}

/// 是否为 git 依赖形态。
fn is_git_spec(lower: &str) -> bool {
    lower.starts_with("git+")
        || lower.starts_with("github:")
        || lower.starts_with("gitlab:")
        || lower.starts_with("bitbucket:")
        || lower.starts_with("git@")
        || lower.starts_with("ssh://")
        || lower.starts_with("git://")
        || (lower.starts_with("http")
            && (lower.contains(".git#") || lower.ends_with(".git") || looks_like_hosted_git_url(lower)))
}

/// 已知 Git 托管站点的 HTTPS URL（含**不带** `.git` 后缀的常见写法）。
///
/// 依据：pnpm 会把这两种形式都规范化为 `github:<owner>/<repo>`（已实测）：
/// `https://github.com/o/r` 与 `https://github.com/o/r.git`。
/// 仅识别**路径恰为两段**（`owner/repo`）的形态，避免把 `.../releases/download/x.tgz`
/// 这类发布附件误判为 git 源。
fn looks_like_hosted_git_url(lower: &str) -> bool {
    const HOSTS: [&str; 3] = [
        "https://github.com/",
        "https://gitlab.com/",
        "https://bitbucket.org/",
    ];
    for host in HOSTS {
        if let Some(rest) = lower.strip_prefix(host) {
            // 去掉可能的 `#ref`，再去掉尾部 `.git`
            let path = rest.split('#').next().unwrap_or(rest);
            let path = path.trim_end_matches('/').trim_end_matches(".git");
            let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            // 恰为 owner/repo 两段
            return segments.len() == 2;
        }
    }
    false
}

/// 把 git spec 归一化为 `git ls-remote` 可用的仓库地址与引用。
///
/// 返回 `(repo_url, ref)`，`ref` 为 `#` 后的分支/tag/sha（可能为空）。
pub fn git_repo_and_ref(spec: &str) -> Option<(String, Option<String>)> {
    let trimmed = spec.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !is_git_spec(&lower) {
        return None;
    }
    let mut body = trimmed;
    if let Some(rest) = body.strip_prefix("git+") {
        body = rest;
    }
    // 短前缀归一化
    let normalized = if let Some(rest) = body.strip_prefix("github:") {
        format!("https://github.com/{rest}")
    } else if let Some(rest) = body.strip_prefix("gitlab:") {
        format!("https://gitlab.com/{rest}")
    } else if let Some(rest) = body.strip_prefix("bitbucket:") {
        format!("https://bitbucket.org/{rest}")
    } else if let Some(rest) = body.strip_prefix("git@") {
        // git@host:owner/repo(.git)
        match rest.split_once(':') {
            Some((host, path)) => format!("ssh://git@{host}/{path}"),
            None => body.to_string(),
        }
    } else {
        body.to_string()
    };
    // 拆分 `#ref`（只认最后一个 `#`，且必须在路径部分之后）
    let (repo, reference) = match normalized.rsplit_once('#') {
        Some((repo, reference)) if !reference.trim().is_empty() => {
            (repo.to_string(), Some(reference.trim().to_string()))
        }
        _ => (normalized.clone(), None),
    };
    Some((repo, reference))
}

/// 引用是否已钉死到 commit sha（7~40 位十六进制）。
pub fn is_pinned_commit(reference: &str) -> bool {
    let value = reference.trim();
    (7..=40).contains(&value.len()) && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// 尽力从 spec 推断包名（npm 与 git 形态；路径形态返回 None）。
///
/// 注意：pnpm 安装 git/路径依赖时依赖名以包自身 `name` 为准，因此安装流程
/// **以依赖集合的前后 diff 为准**，本函数只用于兜底展示与同步规划。
pub fn package_name_from_spec(spec: &str) -> Option<String> {
    let (kind, _) = classify(spec);
    match kind {
        SpecKind::Npm => {
            let trimmed = spec.trim();
            let name = match trimmed.strip_prefix('@') {
                Some(rest) => {
                    // @scope/name@range
                    let (scope, tail) = rest.split_once('/')?;
                    let name = tail.split('@').next()?;
                    format!("@{scope}/{name}")
                }
                None => trimmed.split('@').next()?.to_string(),
            };
            if name.is_empty() {
                None
            } else {
                Some(name)
            }
        }
        SpecKind::Git => {
            let (repo, _) = git_repo_and_ref(spec)?;
            let trimmed = repo.trim_end_matches('/').trim_end_matches(".git");
            let last = trimmed.rsplit('/').next()?;
            if last.is_empty() {
                None
            } else {
                Some(last.to_string())
            }
        }
        _ => None,
    }
}

/// 从 git spec 提取已钉的 commit（无则 None）。
pub fn pinned_commit(spec: &str) -> Option<String> {
    let (_, reference) = git_repo_and_ref(spec)?;
    let reference = reference?;
    if is_pinned_commit(&reference) {
        Some(reference)
    } else {
        None
    }
}

/// 把 package.json 里的依赖值补成可安装 spec。
///
/// `package.json.dependencies` 的 npm 值通常只是版本区间（`^1.2.3`），
/// 而 `dsh plugin add` 需要 `name@range`；git/路径/别名 spec 原样返回。
pub fn full_spec(package: &str, spec: &str) -> String {
    let trimmed = spec.trim();
    let (kind, _) = classify(trimmed);
    if kind != SpecKind::Npm {
        return trimmed.to_string();
    }
    let is_range = trimmed.starts_with(|c: char| {
        c.is_ascii_digit() || matches!(c, '^' | '~' | '>' | '<' | '=' | '*' | 'v')
    }) || matches!(trimmed, "latest" | "next" | "beta" | "alpha");
    if is_range {
        format!("{package}@{trimmed}")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_upstream_and_inhouse() {
        assert_eq!(classify("dsh-cost-meter@^1.7.13"), (SpecKind::Npm, Origin::Upstream));
        assert_eq!(classify("dshmarket"), (SpecKind::Npm, Origin::Upstream));
        assert_eq!(
            classify("github:owner/repo#0123456789abcdef0123456789abcdef01234567"),
            (SpecKind::Git, Origin::Upstream)
        );
        assert_eq!(
            classify("git+https://github.com/o/r.git#main"),
            (SpecKind::Git, Origin::Upstream)
        );
        assert_eq!(classify("link:D:\\work\\my-plugin"), (SpecKind::Path, Origin::InHouse));
        assert_eq!(classify("file:../local-plugin"), (SpecKind::Path, Origin::InHouse));
        assert_eq!(classify("./hello-plugin"), (SpecKind::Path, Origin::InHouse));
        assert_eq!(classify("/opt/plugin"), (SpecKind::Path, Origin::InHouse));
        assert_eq!(classify("C:\\work\\p"), (SpecKind::Path, Origin::InHouse));
        assert_eq!(classify("./dist/pkg-1.0.0.tgz"), (SpecKind::Tarball, Origin::InHouse));
        assert_eq!(
            classify("https://example.com/pkg.tgz"),
            (SpecKind::Unknown, Origin::Unknown)
        );
        assert_eq!(classify(""), (SpecKind::Unknown, Origin::Unknown));
    }

    /// BUG-3（审计）：已知 Git 托管站点的**裸 URL**（无 `.git`）应识别为 git 源。
    ///
    /// 实证：pnpm 11.24 把 `https://github.com/dsh-market/dsh-market` 规范化为
    /// `github:dsh-market/dsh-market` 并成功安装；启动器此前误判为 Unknown
    /// → 不参与 upstream 同步、且无法推断包名。
    #[test]
    fn test_hosted_git_url_without_dot_git() {
        // 裸 URL（用户实际输入形态）→ Git/Upstream
        assert_eq!(
            classify("https://github.com/dsh-market/dsh-market"),
            (SpecKind::Git, Origin::Upstream)
        );
        assert_eq!(
            classify("https://gitlab.com/o/r"),
            (SpecKind::Git, Origin::Upstream)
        );
        assert_eq!(
            classify("https://bitbucket.org/o/r"),
            (SpecKind::Git, Origin::Upstream)
        );
        // 带 .git / 带 #ref 仍然正确
        assert_eq!(
            classify("https://github.com/o/r.git"),
            (SpecKind::Git, Origin::Upstream)
        );
        assert_eq!(
            classify("https://github.com/o/r#abc1234"),
            (SpecKind::Git, Origin::Upstream)
        );
        // 包名可从裸 URL 推断
        assert_eq!(
            package_name_from_spec("https://github.com/dsh-market/dsh-market").as_deref(),
            Some("dsh-market")
        );
        // git 仓库地址规范化（不带 .git 也保持可 ls-remote）
        assert_eq!(
            git_repo_and_ref("https://github.com/dsh-market/dsh-market"),
            Some((
                "https://github.com/dsh-market/dsh-market".to_string(),
                None
            ))
        );
        // 反例：发布附件 / 非 owner-repo 两段形态不得误判为 git
        assert_eq!(
            classify("https://github.com/o/r/releases/download/v1/x.tgz"),
            (SpecKind::Unknown, Origin::Unknown)
        );
        assert_eq!(
            classify("https://example.com/owner/repo"),
            (SpecKind::Unknown, Origin::Unknown)
        );
        // 反例：不在已知托管站点列表内
        assert_eq!(
            classify("https://gitea.example.com/o/r"),
            (SpecKind::Unknown, Origin::Unknown)
        );
    }

    #[test]
    fn test_git_repo_normalization() {
        assert_eq!(
            git_repo_and_ref("github:owner/repo#abc1234"),
            Some((
                "https://github.com/owner/repo".to_string(),
                Some("abc1234".to_string())
            ))
        );
        assert_eq!(
            git_repo_and_ref("git+https://github.com/o/r.git#main"),
            Some((
                "https://github.com/o/r.git".to_string(),
                Some("main".to_string())
            ))
        );
        assert_eq!(
            git_repo_and_ref("git@github.com:o/r.git"),
            Some(("ssh://git@github.com/o/r.git".to_string(), None))
        );
        assert_eq!(git_repo_and_ref("dshmarket"), None);
    }

    #[test]
    fn test_pinned_commit() {
        assert!(is_pinned_commit("0123456789abcdef0123456789abcdef01234567"));
        assert!(is_pinned_commit("abc1234"));
        assert!(!is_pinned_commit("main"));
        assert!(!is_pinned_commit("abc12"));
        assert_eq!(
            pinned_commit("github:o/r#0123456789abcdef0123456789abcdef01234567").as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(pinned_commit("github:o/r#main"), None);
    }

    #[test]
    fn test_full_spec() {
        assert_eq!(full_spec("dsh-cost-meter", "^1.7.13"), "dsh-cost-meter@^1.7.13");
        assert_eq!(full_spec("dshmarket", "latest"), "dshmarket@latest");
        assert_eq!(full_spec("@s/p", "~0.3.17"), "@s/p@~0.3.17");
        // 已经是完整 spec 或非 npm 形态 → 原样
        assert_eq!(full_spec("dsh-cost-meter", "dsh-cost-meter@^1.7.13"), "dsh-cost-meter@^1.7.13");
        assert_eq!(full_spec("p", "github:o/r#abc1234"), "github:o/r#abc1234");
        assert_eq!(full_spec("p", "link:../p"), "link:../p");
    }

    #[test]
    fn test_package_name_from_spec() {        assert_eq!(package_name_from_spec("dshmarket").as_deref(), Some("dshmarket"));
        assert_eq!(
            package_name_from_spec("@scope/pkg@^1.0.0").as_deref(),
            Some("@scope/pkg")
        );
        assert_eq!(
            package_name_from_spec("github:owner/my-plugin#main").as_deref(),
            Some("my-plugin")
        );
        assert_eq!(package_name_from_spec("link:../x"), None);
    }
}
