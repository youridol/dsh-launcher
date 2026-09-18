//! GitHub 通道核心逻辑：查询 release、clone 源码、pnpm 构建
//!
//! 双通道模型（ADR-0001）：GitHub 通道装 v0.1.2-alpha.1 及更新的源码 tag。
//! 全局单版本（ADR-0003）：当前激活版本 = 最近一次安装的版本。
//! 构建产物放 `%LOCALAPPDATA%\dsh-launcher\github-dsh\<version>\`，dsh bin 由 npm 全局链接。
//!
//! v0.1.7 修复：
//! - `hidden_cmd("")` 空命令 bug（`cmd /D /C ""` 退出码 0 但什么都不执行）
//!   → pnpm install/build 从未真正运行 → 安装"假成功/无响应"；改为 `hidden_cmd("pnpm")`
//! - clone/install/build 改用流式执行（core/stream.rs）：逐行写日志 + 推前端实时流
//!   + 进度事件（core/events.rs）

use crate::core::command;
use crate::core::config::AppConfig;
use crate::core::events::InstallPhase;
use crate::core::logging::{LogLevel, Logger};
use crate::core::stream;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// GitHub 仓库
const REPO: &str = "deepseek-ai/deepseek-harness";

/// 定位 git 可执行文件并返回已配 CREATE_NO_WINDOW 的 Command。
///
/// 分发机器修复（v0.4.5）：Git 由启动器"工具链一键安装"装入后，写入的是
/// 注册表 PATH；而启动器进程（及 Explorer 拉起的进程）的 PATH 快照不刷新，
/// 直接 `spawn git` 报 "program not found"（版本列表/克隆全挂）。
/// 这里 PATH 探测失败时回退 Git for Windows 常见安装目录的绝对路径。
fn git_command() -> Result<std::process::Command, String> {
    let resolved = resolve_git_exe().ok_or_else(|| {
        "找不到 git（PATH 中无 git 且未发现 Git for Windows 安装目录）。请在工具链管理中安装 Git".to_string()
    })?;
    Ok(command::hidden(&resolved))
}

/// 解析 git.exe：PATH 探测优先（where git），失败回退常见安装目录
fn resolve_git_exe() -> Option<std::path::PathBuf> {
    let mut probe = command::hidden_cmd("where");
    probe.arg("git");
    if let Ok(out) = probe.output() {
        if out.status.success() {
            let text = crate::core::text::decode(&out.stdout);
            if let Some(line) = text.lines().next() {
                let p = std::path::PathBuf::from(line.trim());
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    for cand in [
        "C:\\Program Files\\Git\\cmd\\git.exe",
        "C:\\Program Files\\Git\\bin\\git.exe",
        "C:\\Program Files (x86)\\Git\\cmd\\git.exe",
    ] {
        let p = std::path::PathBuf::from(cand);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// 若配置了 GitHub Token，通过**环境变量**注入 git 认证
/// （`GIT_CONFIG_COUNT/GIT_CONFIG_KEY_0/GIT_CONFIG_VALUE_0`），避免 PAT 出现在
/// 子进程命令行/日志（审计修复 2.4）。未配置则无操作。
/// 注：该注入方式要求 git ≥ 2.31（2021-03 发布）；更低版本 git 会忽略这些
/// 环境变量、退化为匿名访问（不影响公开仓库）。
fn apply_git_auth(cmd: &mut std::process::Command) {
    let cfg = AppConfig::load();
    let token = cfg.github_token.trim();
    if token.is_empty() {
        return;
    }
    cmd.env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "http.extraheader")
        .env("GIT_CONFIG_VALUE_0", format!("AUTHORIZATION: bearer {token}"));
}

/// 为**任意仓库**构造已配好 git 可执行、Token 与镜像语义的 clone 命令（ADR-0008）。
///
/// 技能导入需要克隆用户给定的任意 URL，而本模块其余函数的仓库是硬编码的 dsh 仓库
/// （`const REPO`）。此处把「git 可执行解析 + Token 注入 + 镜像重写」这三个通用能力
/// 导出复用，**不复制**实现，也**不改变** dsh 仓库路径的既有行为。
///
/// `--depth 1` 是刻意的：技能导入只关心最新内容，不需要全量历史。
pub fn git_clone_command(
    repo_url: &str,
    reference: Option<&str>,
    dest: &std::path::Path,
) -> Result<std::process::Command, String> {
    let mirror = AppConfig::load().github_mirror;
    let effective = resolve_repo_url(repo_url, &mirror);
    let mut cmd = git_command()?;
    apply_git_auth(&mut cmd);
    cmd.arg("clone").arg("--depth").arg("1");
    if let Some(reference) = reference.filter(|r| !r.trim().is_empty()) {
        cmd.arg("--branch").arg(reference.trim());
    }
    cmd.arg("--progress").arg(&effective).arg(dest);
    Ok(cmd)
}

/// 计算实际使用的仓库 URL（仅在 URL 是 `https://github.com/` 且配置了镜像时重写）。
///
/// 对非 GitHub 地址套用 GitHub 镜像前缀只会得到无效 URL，故必须限定前缀。
pub fn resolve_repo_url(repo_url: &str, mirror: &str) -> String {
    let mirror = mirror.trim();
    if mirror.is_empty() {
        return repo_url.to_string();
    }
    const GH_PREFIX: &str = "https://github.com/";
    match repo_url.strip_prefix(GH_PREFIX) {
        Some(rest) => format!("{}/{}", mirror.trim_end_matches('/'), rest),
        None => repo_url.to_string(),
    }
}

/// GitHub 通道源码目录根：%LOCALAPPDATA%\dsh-launcher\github-dsh
pub fn github_dsh_dir() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("dsh-launcher")
        .join("github-dsh")
}

/// 克隆目录固定名（v0.2.3：不再按版本号命名，统一为 deepseek-harness）
const CLONE_DIR: &str = "deepseek-harness";

/// GitHub 通道克隆/构建目录：%LOCALAPPDATA%\dsh-launcher\github-dsh\deepseek-harness
pub fn github_clone_dir() -> PathBuf {
    github_dsh_dir().join(CLONE_DIR)
}

/// GitHub 通道已安装（克隆目录存在且有内容）
pub fn github_installed() -> bool {
    let dir = github_clone_dir();
    dir.is_dir()
        && dir.join("package.json").exists()
        && dir.join("apps").is_dir()
}

/// 查询 GitHub 可用版本（tags）
///
/// 主路径：`git ls-remote --tags`（HTTPS 协议不受 GitHub API 60 次/小时未认证限流影响）。
/// - 附带收益：API /releases 只含 release，ls-remote 能拿到全部 tag（含 rc），列表更全。
/// - 镜像源通过仓库 URL 前缀生效（git 协议路径）；失败时返回带原因的 Err。
/// （历史曾计划 curl API 兜底路径，已废弃——2026 审计修正注释与实现一致。）
pub fn list_releases() -> Result<Vec<String>, String> {
    let cfg = AppConfig::load();
    let repo_url = if cfg.github_mirror.is_empty() {
        format!("https://github.com/{REPO}.git")
    } else {
        format!("{}/{}.git", cfg.github_mirror.trim_end_matches('/'), REPO)
    };

    // 主路径：git ls-remote --tags（不受 GitHub API 限流影响）
    // 配置了 Token 时通过环境变量注入认证头（防限流 / 私有仓库，见 apply_git_auth）
    let mut cmd = git_command()?;
    apply_git_auth(&mut cmd);
    cmd.args(["ls-remote", "--tags", &repo_url]);
    // v0.4.13（审计修复 2.8）：网络查询加超时（此前无任何超时，网络黑洞会永久挂起）
    let out = command::run_with_timeout(cmd, std::time::Duration::from_secs(90))
        .map_err(|e| format!("git ls-remote 执行失败: {e}"))?;
    if out.status.success() {
        let text = crate::core::text::decode(&out.stdout);
        let versions = parse_tags_from_ls_remote(&text);
        if !versions.is_empty() {
            return Ok(versions);
        }
        // ls-remote 成功但无 tag → 仓库无发布，明确提示
        return Ok(Vec::new());
    }
    // git 路径失败（镜像不支持 git 协议等）→ 兜底走 API，并给出诊断信息
    let stderr = crate::core::text::decode(&out.stderr).trim().to_string();
    let hint = if stderr.is_empty() {
        format!("git ls-remote 失败（退出码 {})", out.status.code().unwrap_or(-1))
    } else {
        stderr
    };
    Err(format!("查询 GitHub releases 失败: {hint}"))
}

/// 查询远端引用的 sha（`git ls-remote <repo> <ref>`）。
///
/// ADR-0005 D6 的 upstream 同步用它把 git 依赖推进到目标 commit：
/// 复用本模块的 git 可执行解析、Token 注入与超时兜底，不新增调用路径。
/// `reference` 为空时查询 `HEAD`。
/// @returns 命中的 sha（无匹配返回 None）
pub fn ls_remote_ref(repo_url: &str, reference: &str) -> Result<Option<String>, String> {
    let reference = if reference.trim().is_empty() {
        "HEAD"
    } else {
        reference.trim()
    };
    let mut cmd = git_command()?;
    apply_git_auth(&mut cmd);
    cmd.args(["ls-remote", repo_url, reference]);
    let out = command::run_with_timeout(cmd, std::time::Duration::from_secs(90))
        .map_err(|e| format!("git ls-remote 执行失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git ls-remote 失败: {}",
            crate::core::text::decode(&out.stderr).trim()
        ));
    }
    let text = crate::core::text::decode(&out.stdout);
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(sha), Some(_name)) = (parts.next(), parts.next()) {
            if sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(Some(sha.to_string()));
            }
        }
    }
    Ok(None)
}

/// 从 `git ls-remote --tags` 输出解析 tag 列表（去重、去 ^{} 剥离、降序）
fn parse_tags_from_ls_remote(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let _sha = parts.next()?;
            let refname = parts.next()?;
            // 只接受 tags 引用（HEAD / refs/heads/* 忽略）；形如 refs/tags/dsh-v0.1.2-alpha.1
            let tag = refname
                .strip_prefix("refs/tags/")?
                .trim_end_matches("^{}");
            if tag.is_empty() {
                None
            } else {
                Some(tag.to_string())
            }
        })
        .collect();
    // 去重（ peeled ^{} 与轻量 tag 指向同 sha 时 git 会输出两条）
    tags.sort();
    tags.dedup();
    // v0.4.13（审计修复 2.3）：此前用字典序 b.cmp(a) 排序，跨 10 位版本/预发布
    // 多位数时排序错误（0.9 > 0.10、rc.9 > rc.10）。改为 semver 数值比较降序。
    tags.sort_by(|a, b| cmp_semver_desc(a, b));
    tags
}

/// 预发布标识符：数字段或字符串段（semver 规则：数字 < 字母）
#[derive(Debug, Clone, PartialEq, Eq)]
enum PreId {
    Num(u64),
    Str(String),
}

/// 将 tag 解析为 (核心数字段, 预发布标识符)。
/// 支持 `dsh-v0.1.2-alpha.1` / `v1.2.3` / `1.2.3-beta+build` 等形态；
/// 无法解析数字的输入按 0 处理（不 panic）。
fn parse_semver_parts(v: &str) -> (Vec<u64>, Vec<PreId>) {
    let core = v
        .strip_prefix("dsh-")
        .or_else(|| v.strip_prefix("Dsh-"))
        .or_else(|| v.strip_prefix('v'))
        .or_else(|| v.strip_prefix('V'))
        .unwrap_or(v);
    let (main, pre) = match core.find(['-', '+']) {
        Some(i) => (&core[..i], Some(&core[i + 1..])),
        None => (core, None),
    };
    let nums: Vec<u64> = main
        .split('.')
        .filter_map(|seg| seg.parse::<u64>().ok())
        .collect();
    let pre_ids: Vec<PreId> = pre
        .map(|p| {
            p.split('.')
                .map(|seg| match seg.parse::<u64>() {
                    Ok(n) => PreId::Num(n),
                    Err(_) => PreId::Str(seg.to_ascii_lowercase()),
                })
                .collect()
        })
        .unwrap_or_default();
    (nums, pre_ids)
}

/// semver 降序比较（返回 a 是否应排在 b 之前，即 a 更新则 Less… 见 sort_by 语义）。
/// sort_by 的比较器：返回 Ordering 表示 a 相对 b 的顺序，降序 = 更新版本排前。
///
/// G7（审计 TC-02）：公开给 `commands/version.rs` 用，使**排序只在一端发生**——
/// 此前前端 `lib/version.ts` 另实现了一份等价的 semver 比较（含相同的
/// `rc.9 < rc.10` 修复注释），两端规则有漂移风险。现在 npm/GitHub 两个通道
/// 都在 Rust 侧排好序，前端只负责展示。
pub fn cmp_semver_desc(a: &str, b: &str) -> std::cmp::Ordering {
    cmp_semver_asc(b, a)
}

/// semver 升序比较
fn cmp_semver_asc(a: &str, b: &str) -> std::cmp::Ordering {
    let (nums_a, pre_a) = parse_semver_parts(a);
    let (nums_b, pre_b) = parse_semver_parts(b);
    for i in 0..nums_a.len().max(nums_b.len()) {
        let na = nums_a.get(i).copied().unwrap_or(0);
        let nb = nums_b.get(i).copied().unwrap_or(0);
        match na.cmp(&nb) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    // 核心相同：比较预发布。无预发布（正式版）> 有预发布
    match (pre_a.is_empty(), pre_b.is_empty()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        (false, false) => cmp_pre_ids(&pre_a, &pre_b),
    }
}

/// 预发布标识符列表比较（semver：数字标识符 < 字母标识符；逐段比较；
/// 前缀相同而一个更长时，更长者更大）
fn cmp_pre_ids(a: &[PreId], b: &[PreId]) -> std::cmp::Ordering {
    for i in 0..a.len().max(b.len()) {
        let (Some(x), Some(y)) = (a.get(i), b.get(i)) else {
            // 前缀相同：更长的列表更大（如 alpha < alpha.1）
            return a.len().cmp(&b.len());
        };
        let ord = match (x, y) {
            (PreId::Num(nx), PreId::Num(ny)) => nx.cmp(ny),
            (PreId::Num(_), PreId::Str(_)) => std::cmp::Ordering::Less, // 数字 < 字母
            (PreId::Str(_), PreId::Num(_)) => std::cmp::Ordering::Greater,
            (PreId::Str(sx), PreId::Str(sy)) => sx.cmp(sy),
        };
        match ord {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    a.len().cmp(&b.len())
}

/// 安装指定版本：clone 源码 + pnpm install + build
/// 返回安装到的目录
/// 调用方需提供 Logger（用于日志流 + 进度事件）
pub fn install_version(
    version: &str,
    logger: &Arc<Logger>,
) -> Result<String, String> {
    let cfg = AppConfig::load();
    // 注意：GitHub release tag 命名形如 `dsh-v0.1.2-alpha.1`（带 dsh- 前缀）。
    // list_releases 返回的 tag_name 即为完整 tag，这里必须原样使用，
    // 不能自作主张加 v 前缀（否则会变成 vdsh-v0.1.2-alpha.1 → branch not found）。
    let tag = version.trim().to_string();
    if tag.is_empty() {
        return Err("版本号为空".to_string());
    }
    // v0.2.3：克隆目录固定为 github-dsh\deepseek-harness（不再按版本号命名），
    // 全局单版本语义保留（每次安装覆盖同一目录）
    let dest = github_clone_dir();

    // 已存在则先清理（全局单版本：覆盖旧目录）
    logger.info(&format!("清理旧目录 {}", dest.display()));
    logger.progress("github", InstallPhase::Prepare, 0, "准备安装目录…");
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(&dest).map_err(|e| e.to_string())?;

    // clone（可走镜像）
    let clone_url = if cfg.github_mirror.is_empty() {
        format!("https://github.com/{REPO}.git")
    } else {
        format!("{}/{}.git", cfg.github_mirror.trim_end_matches('/'), REPO)
    };

    logger.info(&format!("开始克隆 {clone_url} (tag={tag})…"));
    logger.progress("github", InstallPhase::Download, 0, "git clone 开始…");
    let cb: Arc<stream::LineCallback> = {
        let logger = Arc::clone(logger);
        Arc::new(move |_lvl, line| {
            // 解析 git clone 的接收进度：Receiving objects:  45%
            if let Some(p) = parse_git_percent(line) {
                logger.progress("github", InstallPhase::Download, p, "正在克隆源码…");
            }
        })
    };
    {
        let mut c = git_command()?;
        apply_git_auth(&mut c);
        c.args([
            "clone",
            "--depth",
            "1",
            "--branch",
            &tag,
            "--progress",
            &clone_url,
            &dest.to_string_lossy(),
        ]);
        stream::run_streamed(logger, c, LogLevel::Info, LogLevel::Warn, Some(cb))
            .map_err(|e| format!("git clone 失败: {e}"))?;
    }
    logger.progress("github", InstallPhase::Download, 100, "源码克隆完成");

    // pnpm install + build（在源码目录内）
    // v0.1.7 修复：此前 hidden_cmd("") 是空命令（cmd /D /C "" 退出码 0 但什么都不做），
    // pnpm 从未执行 → 安装假成功。改为 hidden_cmd("pnpm")。
    logger.progress("github", InstallPhase::Install, 0, "pnpm install 开始…");
    logger.info("开始 pnpm install（安装依赖）…");
    // 依赖安装阶段：按输出行数步进（pnpm 无逐字节百分比），每行 +2%，95% 封顶
    let step = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let cb_install: Arc<stream::LineCallback> = {
        let logger = Arc::clone(logger);
        let step = Arc::clone(&step);
        Arc::new(move |_lvl, line| {
            if line.trim().is_empty() {
                return;
            }
            use std::sync::atomic::Ordering;
            let s = (step.load(Ordering::Relaxed) + 2).min(95);
            step.store(s, Ordering::Relaxed);
            logger.progress("github", InstallPhase::Install, s, "正在安装依赖…");
        })
    };
    // 审计修复 2.1：npm registry 镜像配置此前对 GitHub 通道 pnpm install 不生效，
    // 这里显式注入 --registry（pnpm 与 npm 共用同一 registry 配置语义）。
    let mut pnpm_install_args: Vec<String> = Vec::new();
    if let Some(reg) = crate::core::config::current_npm_registry() {
        pnpm_install_args.push("--registry".to_string());
        pnpm_install_args.push(reg);
    }
    pnpm_install_args.push("install".to_string());
    stream::run_cmd_script(
        logger,
        "pnpm",
        &pnpm_install_args,
        Some(&dest),
        LogLevel::Info,
        Some(cb_install),
    )
    .map_err(|e| format!("pnpm install 失败: {e}"))?;
    logger.progress("github", InstallPhase::Install, 100, "依赖安装完成");

    logger.progress("github", InstallPhase::Build, 0, "pnpm build 开始…");
    logger.info("开始 pnpm build（构建产物）…");
    // 构建阶段：输出行密集，每行 +1%，95% 封顶（避免提前 100）
    let step = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let cb_build: Arc<stream::LineCallback> = {
        let logger = Arc::clone(logger);
        let step = Arc::clone(&step);
        Arc::new(move |_lvl, line| {
            if line.trim().is_empty() {
                return;
            }
            use std::sync::atomic::Ordering;
            let s = (step.load(Ordering::Relaxed) + 1).min(95);
            step.store(s, Ordering::Relaxed);
            logger.progress("github", InstallPhase::Build, s, "正在构建…");
        })
    };
    stream::run_cmd_script(
        logger,
        "pnpm",
        &["run".to_string(), "build".to_string()],
        Some(&dest),
        LogLevel::Info,
        Some(cb_build),
    )
    .map_err(|e| format!("pnpm build 失败: {e}"))?;
    logger.progress("github", InstallPhase::Build, 100, "构建完成");

    // v0.9.6（预防计划 P1-1）：安装后强制收敛 profile。
    //
    // 背景（2026-09-16 审计）：GitHub 通道安装 = 全新 clone + pnpm install/build，
    // 而用户 profile（~/.dsh/profiles/web）的 node_modules 由 pnpm 按旧安装期的
    // 依赖树维护；跨大版本（0.1.5 → 0.1.6）时 workspace 内部包拓扑可能变化，
    // dsh 启动期的 profile 依赖闭环解析（healProfilesModuleFallback）撞上
    // 「半旧半新」的中间态时，个别 bundle 行（实测 dsh-workspace）会静默
    // pending → Sessions/工作区不可用（entries did not activate）。
    //
    // 官方收敛通道：`dsh plugin install`（无 spec）= 在 profile 目录按 lockfile
    // 重新 pnpm install 并对账 dsh.profile.bundles（apps/cli/src/plugin.ts），
    // 幂等、无包变更时近似 no-op。装完即跑一次，确保 profile 与新安装一致后再
    // 让用户启动。
    //
    // 失败策略：只告警不阻断——dsh 本体已构建成功，收敛失败（如 registry 不可达）
    // 不影响启动器可用性；后续「启动健康审查」（process.rs P0-1）会兜底检出
    // pending 并自动重启。
    logger.info("安装后收敛 profile（dsh plugin install，对账依赖与 bundles）…");
    logger.progress("github", InstallPhase::Install, 95, "收敛 profile…");
    let converge_args = vec![
        "plugin".to_string(),
        "--profile".to_string(),
        crate::core::dshhome::MANAGED_PROFILE.to_string(),
        "install".to_string(),
    ];
    match crate::core::profile::run_dsh(&converge_args, crate::core::profile::MUTATION_TIMEOUT) {
        Ok(out) if out.status.success() => {
            logger.info("profile 收敛完成");
        }
        Ok(out) => {
            let stderr = crate::core::text::decode(&out.stderr);
            let tail: String = stderr
                .lines()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            logger.warn(&format!(
                "profile 收敛未成功（退出码 {}），继续安装流程；若启动后功能缺失请查看日志。{}",
                out.status.code().unwrap_or(-1),
                if tail.trim().is_empty() {
                    String::new()
                } else {
                    format!("\nstderr 尾部:\n{tail}")
                }
            ));
        }
        Err(e) => {
            logger.warn(&format!("profile 收敛执行失败（继续安装流程）: {}", e.message));
        }
    }

    // v0.2.3：创建全局 dsh shim（加入 npm 全局 bin 目录），
    // 使 `dsh` 命令全局可用并指向本安装目录（否则启动报 program not found）
    install_global_shim(logger)?;

    logger.progress("github", InstallPhase::Done, 100, "安装完成");

    Ok(format!("GitHub 通道 {version} 构建完成，位于 {}", dest.display()))
}

/// 在 npm 全局 bin 目录创建 dsh.cmd shim，指向 GitHub 安装目录内的 `pnpm dsh`。
/// 目标：`dsh` 命令在任意目录可用（PATH 无需额外配置）。
fn install_global_shim(logger: &Arc<Logger>) -> Result<(), String> {
    let dest = github_clone_dir();
    let bin_dir = npm_prefix_dir();
    fs::create_dir_all(&bin_dir).map_err(|e| format!("创建全局 bin 目录失败: {e}"))?;

    // dsh.cmd：cd 到安装目录后执行 pnpm dsh，透传参数
    let shim = format!(
        "@echo off\r\ncd /d \"{}\"\r\npnpm dsh %*\r\n",
        dest.to_string_lossy()
    );
    let shim_path = bin_dir.join("dsh.cmd");
    fs::write(&shim_path, shim).map_err(|e| format!("写入 dsh.cmd 失败: {e}"))?;
    logger.info(&format!("已创建全局 dsh 命令: {} → 安装目录", shim_path.display()));
    Ok(())
}
/// PATH 中的 dsh 命令的可安全执行性判定结果
///
/// 背景（v0.4.6 修复进程爆炸）：dsh.cmd shim 内容是
/// `cd /d <github 安装目录> && pnpm dsh %*`。当该安装目录被卸载/清空（残留空目录）时，
/// pnpm 在空目录找不到 dsh 项目定义，会把 `dsh` 当作外部命令解析 → 又命中 PATH 中的
/// dsh.cmd → 递归 spawn `cmd → node(pnpm) → cmd → ...`，指数级进程爆炸（实测 10 秒
/// 内积累数百个 node.exe）。因此**执行任何 `dsh` 命令前必须先静态解析其目标**，
/// 本启动器 shim 且目标目录无效时一律短路，绝不执行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DshProbe {
    /// PATH 中无 dsh.cmd
    None,
    /// 本启动器 GitHub shim，且指向的安装目录完整有效（package.json + apps 存在）
    OwnedShimOk,
    /// 本启动器 GitHub shim，但指向的安装目录缺失/不完整（package.json 不存在）
    /// → 执行必触发 pnpm 递归爆炸，调用方必须短路
    OwnedShimBroken,
    /// PATH 中 dsh.cmd 不是本启动器创建（真实 npm 全局包）→ 可安全执行
    Foreign,
}

/// PATH 中 dsh 命令的解析结果：类型 + 可安全执行的 shim 绝对路径。
#[derive(Debug, Clone)]
pub struct ResolvedDsh {
    /// 该 shim 的类型（见 [`DshProbe`]）
    pub kind: DshProbe,
    /// 可安全执行的 dsh.cmd 绝对路径；`OwnedShimBroken` / `None` 时为 None
    /// （此时**禁止**执行 dsh，否则触发 pnpm 递归爆炸）。
    pub shim_path: Option<PathBuf>,
}

/// 遍历 PATH 中全部 `dsh.cmd` 并解析，返回第一个「可安全执行」的（真实 npm 包 shim，
/// 或指向有效安装目录的本启动器 shim）。
///
/// 背景（BUG：npm 通道安装失败）：PATH 中可能存在**多个** dsh.cmd——例如早期版本把
/// 本启动器 GitHub shim 写到了与当前 `npm prefix -g` 不同的 PATH 目录，卸载时漏删；
/// 该陈旧 shim 排在 PATH 首位、指向已删除目录。旧实现只读 `where dsh.cmd` 的**第一行**，
/// 命中陈旧 shim 即判 `OwnedShimBroken` 并短路 → npm 安装成功却仍报「未安装/安装目录
/// 缺失」。此处改为遍历全部条目：损坏的本启动器 shim 被跳过，不遮蔽其后的 npm shim。
pub fn resolve_dsh() -> ResolvedDsh {
    let paths = list_dsh_cmd_paths();
    // 先读取每个候选的内容与目标目录有效性，再交纯函数统一决策（便于单测覆盖
    // 「陈旧 shim 不遮蔽 npm shim」的回归场景）。
    let mut candidates: Vec<(bool, String, bool)> = Vec::with_capacity(paths.len());
    for path in &paths {
        match fs::read_to_string(path) {
            Ok(content) => {
                let dir_valid = extract_shim_target_dir(&content)
                    .map(|dir| dir.join("package.json").exists() && dir.join("apps").is_dir())
                    .unwrap_or(false);
                candidates.push((true, content, dir_valid));
            }
            // 存在但不可读：非本启动器管理，按外部命令处理（保守安全）
            Err(_) => candidates.push((false, String::new(), false)),
        }
    }
    let (kind, idx) = select_usable_shim(candidates.iter().map(|(r, c, d)| (*r, c.as_str(), *d)));
    ResolvedDsh {
        kind,
        shim_path: idx.map(|i| paths[i].clone()),
    }
}

/// 从 PATH 顺序的候选 shim 序列中选出第一个「可安全执行」的（纯决策函数，供单测）。
///
/// 每个候选为 `(内容是否可读, shim 内容, 指向安装目录是否有效)`；返回 `(类型, 选中索引)`。
/// 索引为 None 表示无可用候选（全部损坏或无候选）。
fn select_usable_shim<'a>(
    candidates: impl IntoIterator<Item = (bool, &'a str, bool)>,
) -> (DshProbe, Option<usize>) {
    let mut saw_broken = false;
    for (i, (readable, content, dir_valid)) in candidates.into_iter().enumerate() {
        if !readable {
            return (DshProbe::Foreign, Some(i));
        }
        if !shim_content_is_ours(content) {
            // 真实 npm 全局包 shim（内容不含 github-dsh/pnpm dsh）→ 可安全执行
            return (DshProbe::Foreign, Some(i));
        }
        if extract_shim_target_dir(content).is_some() && dir_valid {
            return (DshProbe::OwnedShimOk, Some(i));
        }
        // 目标安装目录缺失/空目录，或 shim 内容异常：跳过，继续找 PATH 中后续 shim
        saw_broken = true;
    }
    (
        if saw_broken {
            DshProbe::OwnedShimBroken
        } else {
            DshProbe::None
        },
        None,
    )
}

/// 探测 PATH 中 dsh 命令类型（不执行 dsh，仅静态解析 dsh.cmd 内容 + 目录存在性）。
/// 必须在任何 `dsh --version` / `dsh web` 之前调用：
/// 若返回 OwnedShimBroken，继续执行 dsh 会触发 pnpm 递归进程爆炸（见 DshProbe 注释）。
pub fn probe_dsh_command() -> DshProbe {
    resolve_dsh().kind
}

/// 枚举 PATH 中全部 dsh.cmd 绝对路径，按 PATH 顺序去重（大小写不敏感）。
///
/// **统一入口**：`resolve_dsh`（解析可执行 shim）与 `remove_*_github_shims`（清理陈旧
/// shim）共用同一套枚举，避免历史上「一处 `where dsh.cmd`、一处 `env::PATH`」两套来源
/// 不一致（清理看得到、解析看不到，反之亦然）。直接扫描 PATH 目录，**不 spawn `where`**：
/// 行为确定、省一个子进程；为与 `command::hidden_cmd` 的子进程 PATH 保持一致，额外纳入
/// 启动器注入的用户级 node 目录（分发机器上该目录不在本进程 PATH 快照中）。
fn list_dsh_cmd_paths() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(node_dir) = crate::core::pathutil::node_dir_injection() {
        dirs.push(PathBuf::from(node_dir));
    }
    if let Ok(path_var) = std::env::var("PATH") {
        dirs.extend(std::env::split_paths(&path_var).filter(|d| !d.as_os_str().is_empty()));
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let shim = dir.join("dsh.cmd");
        if !shim.is_file() {
            continue;
        }
        let key = shim.to_string_lossy().to_lowercase();
        if seen.insert(key) {
            out.push(shim);
        }
    }
    out
}

/// 一个 dsh.cmd 内容是否为「本启动器 GitHub shim 且已失效」。
/// 失效 = 指向目录已不存在/不完整，或 shim 内容无法解析出目标目录。
fn stale_owned_shim(content: &str) -> bool {
    if !shim_content_is_ours(content) {
        return false;
    }
    match extract_shim_target_dir(content) {
        Some(target) => !(target.join("package.json").exists() && target.join("apps").is_dir()),
        None => true,
    }
}

/// 按谓词删除给定 dsh.cmd 路径中命中的项，返回已删除路径（供日志/测试断言）。
/// 与 [`list_dsh_cmd_paths`] 解耦：测试可直接传入临时目录下的 shim。
fn remove_shims_where(
    logger: &Logger,
    shims: impl IntoIterator<Item = PathBuf>,
    pred: impl Fn(&str) -> bool,
) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    for shim in shims {
        let Ok(content) = fs::read_to_string(&shim) else {
            continue;
        };
        if !pred(&content) {
            continue;
        }
        match fs::remove_file(&shim) {
            Ok(_) => {
                logger.info(&format!("已删除陈旧 dsh shim: {}", shim.display()));
                removed.push(shim);
            }
            Err(e) => logger.warn(&format!(
                "删除陈旧 dsh shim 失败（继续）: {}: {e}",
                shim.display()
            )),
        }
    }
    removed
}

/// 清理 PATH 中**全部**本启动器 GitHub shim。
///
/// 用于「切离 GitHub 通道」（install npm 前置 / 卸载）：此时不再需要任何 GitHub shim，
/// 无论其指向目录当前是否仍有效——即便源码目录删除失败（被占用），残留的自家 shim
/// 也不会再遮蔽真实 npm shim。
pub fn remove_owned_github_shims(logger: &Logger) -> Vec<PathBuf> {
    remove_shims_where(logger, list_dsh_cmd_paths(), shim_content_is_ours)
}

/// 清理 PATH 中**已失效**的本启动器 GitHub shim。
///
/// 用于启动自愈 / 卸载：只删损坏的（指向目录已不存在/不完整），仍指向有效安装目录的
/// 一律不碰。历史遗留（BUG 根因）：早期版本可能把 GitHub shim 写到与当前
/// `npm prefix -g` 不同的 PATH 目录，卸载只删当前 prefix 会漏删 → 陈旧 shim 残留在
/// PATH 首位、遮蔽 npm 全局包生成的 dsh.cmd。
pub fn remove_stale_github_shims(logger: &Logger) -> Vec<PathBuf> {
    remove_shims_where(logger, list_dsh_cmd_paths(), stale_owned_shim)
}

/// 从本启动器 dsh.cmd shim 内容中提取其 cd 目标安装目录（无则 None）
/// 格式：`@echo off\r\ncd /d "<github_dir>"\r\npnpm dsh %*\r\n`
fn extract_shim_target_dir(content: &str) -> Option<std::path::PathBuf> {
    content
        .lines()
        .find_map(|l| {
            let t = l.trim();
            let rest = t.strip_prefix("cd /d \"")?;
            let end = rest.find('\"')?;
            Some(&rest[..end])
        })
        .map(std::path::PathBuf::from)
}

/// 本启动器 dsh.cmd shim 的内容特征：cd 到 github-dsh 安装目录后执行 pnpm dsh %*
/// （install_global_shim 写入；npm 全局包生成的 dsh.cmd 指向 node_modules，不含这两个特征）
fn shim_content_is_ours(content: &str) -> bool {
    content.contains("github-dsh") && content.contains("pnpm dsh")
}

/// npm 全局 prefix 目录（`npm prefix -g`；失败回退到 pnpm 全局目录）
/// 供 GitHub shim 与版本管理面板复用（唯一实现，避免多处重复）
pub fn npm_prefix_dir() -> PathBuf {
    let mut c = command::hidden_cmd("npm");
    c.args(["prefix", "-g"]);
    if let Ok(out) = c.output() {
        if out.status.success() {
            let prefix = crate::core::text::decode(&out.stdout).trim().to_string();
            if !prefix.is_empty() {
                return PathBuf::from(prefix);
            }
        }
    }
    // 回退：pnpm 全局目录
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("pnpm")
}

/// 从 git clone 输出行解析接收百分比（Receiving objects: 45%）
fn parse_git_percent(line: &str) -> Option<u8> {
    let p = line.find('%')?;
    // 往前找数字段
    let before = &line[..p];
    let digits_start = before
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| i + 1)
        .unwrap_or(0);
    let num: u32 = before[digits_start..].trim().parse().ok()?;
    Some(num.min(100) as u8)
}

#[cfg(test)]
mod tests {
    use super::parse_git_percent;
    use super::parse_tags_from_ls_remote;
    use super::shim_content_is_ours;
    use std::fs;

    #[test]
    fn test_parse_git_percent() {
        // 标准 git clone 进度行（含大小/速率）
        assert_eq!(parse_git_percent("Receiving objects:  45% (45/100)"), Some(45));
        assert_eq!(parse_git_percent("Receiving objects: 100% (100/100)"), Some(100));
        assert_eq!(parse_git_percent("Receiving objects: 12%, 3.4 MiB | 1.2 MiB/s"), Some(12));
        // 非进度行不解析
        assert_eq!(parse_git_percent("Cloning into 'foo'..."), None);
        assert_eq!(parse_git_percent("remote: Enumerating objects: 5"), None);
        // 超 100 封顶
        assert_eq!(parse_git_percent("Receiving objects: 105%"), Some(100));
    }

    #[test]
    fn test_parse_tags_from_ls_remote() {
        // 真实 git ls-remote --tags 输出（含 peeled ^{} 重复行）
        let sample = r#"
cd5ef8148158c3a752a658978873241fdf8e2bbc	refs/tags/dsh-v0.1.2-alpha.1
cd5ef8148158c3a752a658978873241fdf8e2bbc	refs/tags/dsh-v0.1.2-alpha.1^{}
528c682e061696f5a160f363f236ecbf53cbd006	refs/tags/dsh-v0.1.1-rc.1
b150a551b8d465e31e418e1b2eaf5e79bbb7d28e	refs/tags/dsh-v0.1.1-rc.2
141eb6fef83422698aef7a981029e843e8161534	refs/tags/dsh-v0.1.0-rc.8
"#;
        let tags = parse_tags_from_ls_remote(sample);
        // 去重（dsh-v0.1.2-alpha.1 只出现一次）+ 去 ^{} 后缀
        assert_eq!(tags.len(), 4, "应得到 4 个去重 tag: {tags:?}");
        assert!(tags.contains(&"dsh-v0.1.2-alpha.1".to_string()));
        assert!(tags.contains(&"dsh-v0.1.1-rc.1".to_string()));
        assert!(tags.contains(&"dsh-v0.1.1-rc.2".to_string()));
        assert!(tags.contains(&"dsh-v0.1.0-rc.8".to_string()));
        // 降序：dsh-v0.1.2 应在 dsh-v0.1.0 前
        let idx_12 = tags.iter().position(|t| t == "dsh-v0.1.2-alpha.1").unwrap();
        let idx_10 = tags.iter().position(|t| t == "dsh-v0.1.0-rc.8").unwrap();
        assert!(idx_12 < idx_10, "降序排列: {tags:?}");

        // 空输出 → 空列表
        assert!(parse_tags_from_ls_remote("").is_empty());
        // 无 tags 前缀的行忽略
        assert!(parse_tags_from_ls_remote("abc123	HEAD\n").is_empty());
    }

    #[test]
    fn test_tag_sort_semver_numeric() {
        use super::parse_tags_from_ls_remote;
        // v0.4.13（审计修复 2.3）：跨 10 位与多位数预发布的排序回归
        let sample = "\
a\trefs/tags/dsh-v0.1.1-rc.9
b\trefs/tags/dsh-v0.1.1-rc.10
c\trefs/tags/dsh-v0.10.0
d\trefs/tags/dsh-v0.9.0
e\trefs/tags/dsh-v0.1.2-alpha.1
f\trefs/tags/dsh-v0.1.1-rc.2
";
        let tags = parse_tags_from_ls_remote(sample);
        let expect = [
            "dsh-v0.10.0",
            "dsh-v0.9.0",
            "dsh-v0.1.2-alpha.1",
            "dsh-v0.1.1-rc.10",
            "dsh-v0.1.1-rc.9",
            "dsh-v0.1.1-rc.2",
        ];
        assert_eq!(tags, expect, "semver 降序应正确: {tags:?}");
    }

    /// G7（审计 TC-02）：`cmp_semver_desc` 现已公开给 npm 通道排序，
    /// 直接对比较器本身加回归（不再仅靠 tags 解析的间接覆盖）。
    #[test]
    fn test_cmp_semver_desc_npm_channel_order() {
        use super::cmp_semver_desc;
        let mut versions = vec![
            "0.1.1-rc.9",
            "0.1.1-rc.10",
            "0.10.0",
            "0.9.0",
            "0.1.2-alpha.1",
            "0.1.1-rc.2",
        ];
        versions.sort_by(|a, b| cmp_semver_desc(a, b));
        assert_eq!(
            versions,
            vec!["0.10.0", "0.9.0", "0.1.2-alpha.1", "0.1.1-rc.10", "0.1.1-rc.9", "0.1.1-rc.2"],
            "npm 通道同一套 semver 降序规则（与 GitHub tags 一致）"
        );
        // 正式版 > 同号预发布
        assert!(cmp_semver_desc("0.2.0", "0.2.0-rc.1").is_lt());
        // 数字段 < 字母段
        assert!(cmp_semver_desc("0.1.1-rc.2", "0.1.1-alpha.1").is_lt());
    }

    #[test]
    fn test_shim_content_is_ours() {
        // 本启动器 install_global_shim 写入的 dsh.cmd（cd 到 github-dsh 安装目录 + pnpm dsh %*）
        let ours = r#"@echo off
cd /d "C:\Users\Administrator\AppData\Local\dsh-launcher\github-dsh\deepseek-harness"
pnpm dsh %*
"#;
        assert!(shim_content_is_ours(ours), "本启动器 GitHub shim 应被识别");
        // npm 全局包（@deepseek-ai/dsh）生成的 dsh.cmd：指向 node_modules，不含两个特征
        let npm_shim = r#"@ECHO off
SETLOCAL
CALL "C:\Users\Administrator\AppData\Local\dsh-launcher\toolchain\node\node_modules\@deepseek-ai\dsh\bin\dsh.cmd" %*
"#;
        assert!(!shim_content_is_ours(npm_shim), "npm 全局 shim 不应误判为本启动器 shim");
        // 内容不可读 / 无特征内容
        assert!(!shim_content_is_ours(""));
        assert!(!shim_content_is_ours("echo dsh"));
    }

    #[test]
    fn test_extract_shim_target_dir() {
        use super::extract_shim_target_dir;
        // 标准 shim：cd /d "<dir>"
        let ours = "@echo off\r\ncd /d \"C:\\Users\\Administrator\\AppData\\Local\\dsh-launcher\\github-dsh\\deepseek-harness\"\r\npnpm dsh %*\r\n";
        let dir = extract_shim_target_dir(ours).expect("应提取到安装目录");
        assert_eq!(
            dir.to_string_lossy(),
            "C:\\Users\\Administrator\\AppData\\Local\\dsh-launcher\\github-dsh\\deepseek-harness"
        );
        // 无 cd 行 / 格式异常 → None（判定为损坏 shim）
        assert!(extract_shim_target_dir("@echo off\r\npnpm dsh %*\r\n").is_none());
        assert!(extract_shim_target_dir("").is_none());
        // 非本启动器 npm shim（无 cd /d 行）→ None
        let npm_shim = "@ECHO off\r\nSETLOCAL\r\nCALL \"...node_modules\\@deepseek-ai\\dsh\\bin\\dsh.cmd\" %*\r\n";
        assert!(extract_shim_target_dir(npm_shim).is_none());
    }

    #[test]
    fn test_select_usable_shim_skips_stale_owned_shim() {
        use super::select_usable_shim;
        use super::DshProbe;
        let broken_owned = "@echo off\r\ncd /d \"C:\\Users\\A\\Local\\dsh-launcher\\github-dsh\\deepseek-harness\"\r\npnpm dsh %*\r\n";
        let npm = "@ECHO off\r\nSETLOCAL\r\nCALL \"...node_modules\\@deepseek-ai\\dsh\\bin\\dsh.cmd\" %*\r\n";

        // 回归场景：PATH 首位是陈旧损坏的 GitHub shim，其后才是 npm 全局包 shim →
        // 旧实现只读首行会误判 OwnedShimBroken，导致 npm 安装成功却报「未安装」。
        // 修复后应跳过损坏项，选中 npm shim（Foreign）。
        let (kind, idx) = select_usable_shim([
            (true, broken_owned, false), // 指向已删除目录
            (true, npm, false),
        ]);
        assert_eq!(kind, DshProbe::Foreign, "应选中 npm shim 而非被陈旧 shim 遮蔽");
        assert_eq!(idx, Some(1));

        // 全部为损坏的 GitHub shim → 仍判 OwnedShimBroken（保留防递归爆炸短路）
        let (kind, idx) = select_usable_shim([
            (true, broken_owned, false),
            (true, broken_owned, false),
        ]);
        assert_eq!(kind, DshProbe::OwnedShimBroken);
        assert_eq!(idx, None);

        // 有效 GitHub shim 在前 → 选中 OwnedShimOk
        let (kind, idx) = select_usable_shim([(true, broken_owned, true), (true, npm, false)]);
        assert_eq!(kind, DshProbe::OwnedShimOk);
        assert_eq!(idx, Some(0));

        // 无候选 → None
        let (kind, idx) = select_usable_shim([]);
        assert_eq!(kind, DshProbe::None);
        assert_eq!(idx, None);
    }

    #[test]
    fn test_stale_owned_shim_predicate() {
        use super::stale_owned_shim;
        let broken_owned = "@echo off\r\ncd /d \"C:\\Users\\A\\Local\\dsh-launcher\\github-dsh\\deepseek-harness\"\r\npnpm dsh %*\r\n";
        let npm = "@ECHO off\r\nSETLOCAL\r\nCALL \"...node_modules\\@deepseek-ai\\dsh\\bin\\dsh.cmd\" %*\r\n";
        // npm shim 永不算「陈旧自家 shim」
        assert!(!stale_owned_shim(npm));
        assert!(!stale_owned_shim(""));
        // 自家 shim 但无法解析目标目录（含 github-dsh 特征、无 cd /d 行）→ 陈旧
        assert!(stale_owned_shim("@echo off\r\nrem github-dsh\r\npnpm dsh %*\r\n"));
        // 自家 shim 指向不存在的目录 → 陈旧
        assert!(stale_owned_shim(broken_owned));
    }

    /// 文件系统级回归：删除函数必须「删陈旧自家 shim / 保留 npm shim / 保留有效自家 shim」。
    ///
    /// 复现 npm 通道 BUG 的清理语义：PATH 首位陈旧自家 shim → 必须删；其后的 npm shim
    /// → 必须保留；仍指向有效安装目录的自家 shim → 启动自愈不得误删。
    #[test]
    fn test_remove_shims_filesystem() {
        use super::{remove_shims_where, shim_content_is_ours, stale_owned_shim};
        let logger = crate::core::logging::Logger::init();
        let root = std::env::temp_dir().join(format!("dsh-shim-clean-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let stale = root.join("stale").join("dsh.cmd");
        let npm = root.join("npm").join("dsh.cmd");
        let valid = root.join("valid").join("dsh.cmd");
        // 有效自家 shim：目标目录含 package.json + apps（且路径含 github-dsh 特征）
        let valid_target = root.join("github-dsh").join("deepseek-harness");
        fs::create_dir_all(valid_target.join("apps")).unwrap();
        fs::write(valid_target.join("package.json"), "{}").unwrap();
        for dir in [stale.parent().unwrap(), npm.parent().unwrap(), valid.parent().unwrap()] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(
            &stale,
            "@echo off\r\ncd /d \"C:\\nonexistent\\dsh-launcher\\github-dsh\\deepseek-harness\"\r\npnpm dsh %*\r\n",
        )
        .unwrap();
        fs::write(
            &npm,
            "@ECHO off\r\nSETLOCAL\r\nCALL \"...node_modules\\@deepseek-ai\\dsh\\bin\\dsh.cmd\" %*\r\n",
        )
        .unwrap();
        fs::write(
            &valid,
            format!(
                "@echo off\r\ncd /d \"{}\"\r\npnpm dsh %*\r\n",
                valid_target.display()
            ),
        )
        .unwrap();

        // 启动自愈语义：只删指向已失效目录的自家 shim
        let removed = remove_shims_where(
            &logger,
            [stale.clone(), npm.clone(), valid.clone()],
            stale_owned_shim,
        );
        assert_eq!(removed, vec![stale.clone()], "只应删除指向已失效目录的自家 shim");
        assert!(!stale.exists(), "陈旧 shim 应已删除");
        assert!(npm.exists(), "npm shim 必须保留");
        assert!(valid.exists(), "有效自家 shim 必须保留");

        // 切离 GitHub 通道语义：删全部自家 shim（npm shim 仍保留）
        let removed = remove_shims_where(
            &logger,
            [npm.clone(), valid.clone()],
            shim_content_is_ours,
        );
        assert_eq!(removed, vec![valid.clone()]);
        assert!(npm.exists(), "npm shim 永不被 remove_owned_github_shims 删除");

        let _ = fs::remove_dir_all(&root);
    }
}
