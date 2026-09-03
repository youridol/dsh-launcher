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
fn cmp_semver_desc(a: &str, b: &str) -> std::cmp::Ordering {
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
    let bin_dir = npm_global_bin_dir();
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

/// 全局 dsh.cmd shim 路径（卸载时清理用）
pub fn global_shim_path() -> Option<PathBuf> {
    let p = npm_global_bin_dir().join("dsh.cmd");
    if p.exists() {
        Some(p)
    } else {
        None
    }
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

/// 探测 PATH 中 dsh 命令类型（不执行 dsh，仅静态解析 dsh.cmd 内容 + 目录存在性）。
/// 必须在任何 `dsh --version` / `dsh web` 之前调用：
/// 若返回 OwnedShimBroken，继续执行 dsh 会触发 pnpm 递归进程爆炸（见 DshProbe 注释）。
pub fn probe_dsh_command() -> DshProbe {
    let mut probe = command::hidden_cmd("where");
    probe.arg("dsh.cmd");
    let Ok(out) = probe.output() else {
        return DshProbe::None;
    };
    if !out.status.success() {
        return DshProbe::None;
    }
    let text = crate::core::text::decode(&out.stdout);
    let Some(first) = text.lines().next() else {
        return DshProbe::None;
    };
    let path = PathBuf::from(first.trim());
    let Ok(content) = fs::read_to_string(&path) else {
        // 存在但不可读：非本启动器管理，按外部命令处理（保守安全，不执行判断依据不足）
        return DshProbe::Foreign;
    };
    if !shim_content_is_ours(&content) {
        // 真实 npm 全局包 shim（内容不含 github-dsh/pnpm dsh）
        return DshProbe::Foreign;
    }
    // 本启动器 shim：解析其指向的安装目录（内容中 cd /d "<dir>"）
    // 格式：@echo off\r\ncd /d "<github_dir>"\r\npnpm dsh %*\r\n
    let Some(dir) = extract_shim_target_dir(&content) else {
        // shim 内容异常（无法解析目标目录）：视为损坏，禁止执行
        return DshProbe::OwnedShimBroken;
    };
    let install_valid = dir.join("package.json").exists() && dir.join("apps").is_dir();
    if install_valid {
        DshProbe::OwnedShimOk
    } else {
        // 目标安装目录缺失/空目录：执行 dsh 会触发 pnpm 递归爆炸，判定为损坏
        DshProbe::OwnedShimBroken
    }
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

/// 定位 npm 全局 bin 目录（Windows：npm 把 .cmd 直接放 prefix 目录，PATH 条目即 prefix）
fn npm_global_bin_dir() -> PathBuf {
    npm_prefix_dir()
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
}
