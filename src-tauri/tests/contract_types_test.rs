//! 前后端类型契约快照测试（审计 TC-03）
//!
//! ## 为什么需要
//!
//! `src/lib/tauri.ts` 的类型与 `src-tauri/src/commands/**` 的 DTO 全靠**人工注释**
//! 对齐（如 `// 对应 Rust DshStatus`）。审计 TC-03 指出：命令与类型数量增长后，
//! 漏改一侧不会被任何门禁拦住 —— Rust 改名 → 前端读到 `undefined`，Rust 加字段 →
//! 前端悄悄丢弃，二者都不会报错。
//!
//! ## 为什么不引入 ts-rs / specta
//!
//! 本仓**没有 TS 测试基建**（无 vitest/jest），引入绑定生成器需要：新依赖 +
//! 生成物落盘策略 + 与手写 `tauri.ts`（含函数封装与 JSDoc）的共存规则。
//! 收益与“字段集合是否一致”这一核心目标相比不成比例。故本测试直接用**源码文本**
//! 做双侧对账：零新依赖、零生产代码侵入、失败信息可定位到具体 DTO。
//!
//! ## 覆盖范围与已知边界
//!
//! - 覆盖：每个 DTO 的**字段集合**（Rust `snake_case` + `rename_all` → 前端 `camelCase`）。
//! - 覆盖：`rename_all` 的取值语义（camelCase / lowercase / kebab-case）。
//! - **不**覆盖：字段**类型**（`Option<T>` ↔ `T | null` 的深度匹配）、可选性（`?`）、
//!   嵌套泛型。这些需要真正的绑定生成或 JSON Schema 快照，属后续增强。
//! - 配对表（`PAIRS`）是**显式**的：Rust 侧名字与 TS 侧名字不同的（如
//!   `ToggleReport` ↔ `SkillToggleReport`）必须列明，改名即测试失败——这是有意的，
//!   因为它强制维护者确认“是不是漏改了另一侧”。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 仓库根（`src-tauri/..`）
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有上级目录")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()))
}

// ==================== Rust 侧解析 ====================

/// 一个 Rust DTO 的字段集合（已按 `rename_all` 转成前端视角的 JSON 键名）。
#[derive(Debug, Clone)]
struct RustDto {
    /// JSON 键名集合（已应用 rename_all）
    fields: BTreeSet<String>,
    /// 该 struct/enum 上声明的 `rename_all` 值（缺省 camelCase 时为 None）
    rename_all: Option<String>,
}

/// 把 Rust 标识符按 serde 的 `rename_all` 规则转为 JSON 键名。
fn apply_rename_all(ident: &str, rule: &str) -> String {
    match rule {
        "camelCase" => {
            let mut out = String::new();
            let mut upper = false;
            for c in ident.chars() {
                if c == '_' {
                    upper = true;
                } else if upper {
                    out.extend(c.to_uppercase());
                    upper = false;
                } else {
                    out.push(c);
                }
            }
            out
        }
        "lowercase" => ident.to_ascii_lowercase(),
        "kebab-case" => ident.replace('_', "-"),
        "SCREAMING_SNAKE_CASE" => ident.to_ascii_uppercase(),
        other => panic!("未支持的 rename_all 规则: {other}"),
    }
}

/// 从 Rust 源码中提取 `pub struct <name>` 的字段（应用其 `rename_all`）。
///
/// 约定：本仓 DTO 一律 `#[derive(..., Serialize)]` + `#[serde(rename_all = "...")]`
/// 且无 `serde(flatten)`。若将来出现逐字段 `#[serde(rename)]`，本函数需同步增强
/// （测试会因字段集合不匹配而失败，从而被注意到）。
fn rust_fields(src: &str, name: &str) -> RustDto {
    let needle = format!("pub struct {name}");
    let start = src
        .find(&needle)
        .unwrap_or_else(|| panic!("Rust 侧找不到 pub struct {name}"));
    // 向后找到属性块（#[derive...] / #[serde(...)] 在 struct 之前）
    let head_start = src[..start].rfind("\n\n").map(|i| i + 1).unwrap_or(0);
    let head = &src[head_start..start];
    let rename_all = regex_like_capture(head, "rename_all");

    // 取 struct 体（第一个 '{' 到配对的 '}'）
    let body_start = src[start..].find('{').map(|i| start + i).unwrap();
    let body_end = src[body_start..]
        .find("\n}")
        .map(|i| body_start + i)
        .unwrap_or_else(|| panic!("struct {name} 未找到结束花括号"));
    let body = &src[body_start + 1..body_end];

    let rule = rename_all.as_deref().unwrap_or("camelCase");
    let mut fields = BTreeSet::new();
    for line in body.lines() {
        let line = line.trim();
        // 只取 `pub <ident>:` 开头的字段声明行（跳过注释与嵌套结构）
        let Some(rest) = line.strip_prefix("pub ") else {
            continue;
        };
        let Some((ident, _)) = rest.split_once(':') else {
            continue;
        };
        let ident = ident.trim();
        if ident.is_empty() || ident.contains(char::is_whitespace) {
            continue;
        }
        fields.insert(apply_rename_all(ident, rule));
    }
    RustDto { fields, rename_all }
}

/// 极简 `rename_all = "..."` 提取（避免引入 regex 依赖）。
fn regex_like_capture(head: &str, key: &str) -> Option<String> {
    for line in head.lines().rev().take(6) {
        let line = line.trim();
        if !line.starts_with("#[") {
            // 属性块以上的普通注释/代码：停止
            if line.starts_with("///") || line.starts_with("//") || line.is_empty() {
                continue;
            }
            break;
        }
        if let Some(pos) = line.find(key) {
            let after = &line[pos + key.len()..];
            let after = after.trim_start().trim_start_matches('=').trim_start();
            let quote = after.chars().next()?;
            if quote == '"' {
                let rest = &after[1..];
                if let Some(end) = rest.find('"') {
                    return Some(rest[..end].to_string());
                }
            }
        }
    }
    None
}

// ==================== TS 侧解析 ====================

/// 从 `src/lib/tauri.ts` 提取 `export interface <name>` 的字段名集合。
fn ts_fields(src: &str, name: &str) -> BTreeSet<String> {
    let needle = format!("export interface {name} ");
    let needle_alt = format!("export interface {name} {{");
    let start = src
        .find(&needle)
        .or_else(|| src.find(&needle_alt))
        .unwrap_or_else(|| panic!("TS 侧找不到 export interface {name}"));
    let body_start = src[start..]
        .find('{')
        .map(|i| start + i)
        .unwrap_or_else(|| panic!("interface {name} 缺少 '{{'"));
    let mut depth = 0i32;
    let mut body_end = None;
    for (i, c) in src[body_start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    body_end = Some(body_start + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let body_end = body_end.unwrap_or_else(|| panic!("interface {name} 未闭合"));
    let body = &src[body_start + 1..body_end];

    let mut fields = BTreeSet::new();
    let mut depth = 0i32;
    for raw in body.lines() {
        let line = raw.trim();
        if line.starts_with("//") || line.starts_with("/*") || line.starts_with('*') {
            continue;
        }
        // 跳过嵌套结构（对象/数组字面量内的行）
        if depth == 0 {
            if let Some(rest) = strip_member(line) {
                fields.insert(rest);
            }
        }
        depth += line.matches(['{', '[', '(']).count() as i32;
        depth -= line.matches(['}', ']', ')']).count() as i32;
    }
    fields
}

/// 从一行 TS 成员声明提取字段名（`name: T;` / `name?: T;` / `name: T | null;`）。
fn strip_member(line: &str) -> Option<String> {
    if line.starts_with('}') || line.starts_with(']') {
        return None;
    }
    let (name, _) = line.split_once(':')?;
    let name = name.trim().trim_end_matches('?').trim();
    if name.is_empty()
        || name.contains(char::is_whitespace)
        || name.contains('"')
        || name.contains('\'')
    {
        return None;
    }
    Some(name.to_string())
}

// ==================== 配对表 ====================

/// `(Rust 所在文件, Rust 类型名, TS 类型名)`。
///
/// 显式列出两侧名字不同的配对：改名即失败，强制确认另一侧是否同步。
const PAIRS: &[(&str, &str, &str)] = &[
    // ---- commands ----
    ("src-tauri/src/commands/config.rs", "ConfigView", "AppConfig"),
    ("src-tauri/src/commands/logs.rs", "LogFile", "LogFile"),
    (
        "src-tauri/src/commands/toolchain.rs",
        "ToolchainItem",
        "ToolchainItem",
    ),
    (
        "src-tauri/src/commands/toolchain.rs",
        "BatchResult",
        "BatchResult",
    ),
    (
        "src-tauri/src/commands/version.rs",
        "DshVersion",
        "DshVersion",
    ),
    (
        "src-tauri/src/commands/version.rs",
        "InstallPaths",
        "InstallPaths",
    ),
    // ---- plugin（ADR-0005）----
    ("src-tauri/src/core/plugin/mod.rs", "OpResult", "OpResult"),
    ("src-tauri/src/core/plugin/mod.rs", "RowView", "RowView"),
    ("src-tauri/src/core/plugin/mod.rs", "PluginView", "PluginView"),
    ("src-tauri/src/core/plugin/mod.rs", "PluginList", "PluginList"),
    ("src-tauri/src/core/plugin/mod.rs", "SyncReport", "SyncReport"),
    (
        "src-tauri/src/core/plugin/mod.rs",
        "SyncItemResult",
        "SyncItemResult",
    ),
    (
        "src-tauri/src/core/plugin/registry.rs",
        "PluginSource",
        "PluginSource",
    ),
    (
        "src-tauri/src/core/plugin/registry.rs",
        "SyncRecord",
        "SyncRecord",
    ),
    // ---- skill（ADR-0007 / 0008）----
    ("src-tauri/src/core/skill/scan.rs", "SkillEntry", "SkillEntry"),
    (
        "src-tauri/src/core/skill/scan.rs",
        "RootStatus",
        "SkillRootStatus",
    ),
    ("src-tauri/src/core/skill/scan.rs", "SkillList", "SkillList"),
    (
        "src-tauri/src/core/skill/manage.rs",
        "ToggleReport",
        "SkillToggleReport",
    ),
    (
        "src-tauri/src/core/skill/manage.rs",
        "DeleteReport",
        "SkillDeleteReport",
    ),
    (
        "src-tauri/src/core/skill/import.rs",
        "SkillImportPlan",
        "SkillImportPlan",
    ),
    (
        "src-tauri/src/core/skill/import.rs",
        "FileDiff",
        "SkillFileDiff",
    ),
    (
        "src-tauri/src/core/skill/import.rs",
        "ImportReport",
        "SkillImportReport",
    ),
    (
        "src-tauri/src/core/skill/import.rs",
        "ImportItem",
        "SkillImportItem",
    ),
    (
        "src-tauri/src/core/skill/import.rs",
        "BatchItemResult",
        "SkillBatchItemResult",
    ),
    (
        "src-tauri/src/core/skill/import.rs",
        "BatchImportReport",
        "SkillBatchImportReport",
    ),
    (
        "src-tauri/src/core/skill/source.rs",
        "SourceRecord",
        "SkillSourceRecord",
    ),
    (
        "src-tauri/src/core/skill/source.rs",
        "SourceRegistry",
        "SkillSourceRegistry",
    ),
    (
        "src-tauri/src/core/skill/update.rs",
        "SourceCheck",
        "SkillSourceCheck",
    ),
    (
        "src-tauri/src/core/skill/update.rs",
        "UpdateCheckReport",
        "SkillUpdateCheckReport",
    ),
    (
        "src-tauri/src/core/skill/editor.rs",
        "OpenReport",
        "SkillOpenReport",
    ),
    // ---- mcp（ADR-0006）----
    (
        "src-tauri/src/core/mcp/mod.rs",
        "McpListResult",
        "McpListResult",
    ),
    (
        "src-tauri/src/core/mcp/state.rs",
        "McpServerView",
        "McpServerView",
    ),
    (
        "src-tauri/src/core/mcp/prereq.rs",
        "PrereqView",
        "McpPrereq",
    ),
];

// ==================== 测试 ====================

/// 全部 DTO：Rust `snake_case` 字段经 `rename_all` 后，必须与 TS 字段集合**完全一致**。
#[test]
fn 前后端_dto_字段集合一致() {
    let ts_src = read("src/lib/tauri.ts");
    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for (file, rust_name, ts_name) in PAIRS {
        let rust_src = read(file);
        let rust = rust_fields(&rust_src, rust_name);
        let ts = ts_fields(&ts_src, ts_name);

        let missing_in_ts: Vec<_> = rust.fields.difference(&ts).cloned().collect();
        let missing_in_rust: Vec<_> = ts.difference(&rust.fields).cloned().collect();

        if !missing_in_ts.is_empty() || !missing_in_rust.is_empty() {
            failures.push(format!(
                "{rust_name} ({file}) ↔ {ts_name}:\n    前端缺字段: {missing_in_ts:?}\n    后端缺字段: {missing_in_rust:?}"
            ));
        }
        checked += 1;
    }

    assert!(
        failures.is_empty(),
        "以下 DTO 的前后端字段集合不一致（共检查 {checked} 组）：\n  - {}",
        failures.join("\n  - ")
    );
    assert!(checked >= 30, "配对表过小（仅 {checked} 组），疑似解析失效");
}

/// 反向哨兵：确认解析器**真的在工作**（否则“全部一致”可能是解析返回空集导致的假绿）。
#[test]
fn 契约解析器非空校验() {
    let ts_src = read("src/lib/tauri.ts");
    let skill = ts_fields(&ts_src, "SkillEntry");
    assert!(
        skill.contains("whenToUse") && skill.contains("overriddenBy"),
        "TS 解析器未能提取 SkillEntry 的 camelCase 字段: {skill:?}"
    );
    assert!(skill.len() >= 12, "SkillEntry 字段数异常: {}", skill.len());

    let rust_src = read("src-tauri/src/core/skill/scan.rs");
    let rust = rust_fields(&rust_src, "SkillEntry");
    assert!(
        rust.fields.contains("whenToUse") && rust.fields.contains("overriddenBy"),
        "Rust 解析器未能把 snake_case 转成 camelCase: {:?}",
        rust.fields
    );
    assert_eq!(rust.rename_all.as_deref(), Some("camelCase"));
}

/// 人为篡改必须被检出（防止测试退化为恒真）。
#[test]
fn 契约测试能检出人为漂移() {
    let rust_src = read("src-tauri/src/core/skill/scan.rs");
    let dto = rust_fields(&rust_src, "SkillEntry");
    // 模拟“前端漏了一个字段”：从 Rust 集合里删掉一个，断言集合确实不同
    let mut mutated = dto.fields.clone();
    mutated.remove("whenToUse");
    assert_ne!(mutated, dto.fields, "移除字段后集合应不同（测试自检）");

    let ts_src = read("src/lib/tauri.ts");
    let ts = ts_fields(&ts_src, "SkillEntry");
    let diff: Vec<_> = ts.difference(&mutated).cloned().collect();
    assert_eq!(
        diff,
        vec!["whenToUse".to_string()],
        "应精确报出缺失字段，实际: {diff:?}"
    );
}

/// `rename_all` 取值语义（lowercase / kebab-case 也用于枚举，这里覆盖转换函数本身）。
#[test]
fn rename_all_转换规则正确() {
    assert_eq!(apply_rename_all("when_to_use", "camelCase"), "whenToUse");
    assert_eq!(apply_rename_all("is_symlink", "camelCase"), "isSymlink");
    assert_eq!(apply_rename_all("trashed_to", "camelCase"), "trashedTo");
    // 注意：`lowercase` 用于**枚举变体**（Rust 写成 `StreamableHttp`，非 snake_case）
    assert_eq!(
        apply_rename_all("StreamableHttp", "lowercase"),
        "streamablehttp"
    );
    assert_eq!(
        apply_rename_all("requires_restart", "kebab-case"),
        "requires-restart"
    );
    // 无下划线时保持不变
    assert_eq!(apply_rename_all("port", "camelCase"), "port");
}
