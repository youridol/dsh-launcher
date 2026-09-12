//! 外部程序打开（ADR-0008 D-Editor）
//!
//! ## 为什么不让前端传路径
//!
//! Tauri v2 的 ACL **只约束前端 IPC 调用**，Rust 侧调用插件 API 不受权限限制。
//! 若改由前端 `openPath(任意路径)` 打开文件，就必须在 capability 里授予
//! `opener:allow-open-path` 并放宽 scope —— 等于把「打开用户机器上任意文件」的能力
//! 交给 webview。
//!
//! 本模块取相反做法：前端只能传一个**闭集枚举**（`OpenTarget`），具体路径由 Rust
//! 从可信 helper 推导。因此 **`capabilities/` 无需任何改动**，且路径穿越在结构上
//! 不可能发生（与 ADR-0007 D10「前端只作身份声明」的纪律一致）。
//!
//! ## 退回链
//!
//! ① 若配置了 `editor_command` → 用它启动（支持 `code --wait` 这类带参数命令行，
//!    可执行文件走 PATH 解析）；
//! ② 否则 → 用**系统默认关联程序**打开（Windows：`ShellExecuteW`，见下方说明）。
//!
//! 两者都失败时返回具名错误，由前端提示用户去设置里配置编辑器。

use crate::core::config::AppConfig;
use crate::core::dshhome;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// 允许打开的目标（**闭集**：前端无法指定任意路径）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenTarget {
    /// `<agentsHome>/AGENTS.md`（官方用户全局指令的共享真源）
    AgentsMd,
    /// `<agentsHome>/CONTEXT.md`（**dsh 不读**，agent/技能侧的约定词表）
    ContextMd,
    /// `<agentsHome>/skills` 技能根目录（在资源管理器中定位）
    SkillsRoot,
}

/// 打开结果
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenReport {
    /// 实际打开的路径
    pub path: String,
    /// 实际使用的打开方式：`editor`（配置的编辑器）或 `system`（系统默认关联程序）
    pub via: String,
    /// 该目标文件是否本次被创建（此前不存在时为 true）
    pub created: bool,
}

/// 打开失败
#[derive(Debug)]
pub enum OpenError {
    /// 找不到目标（且创建失败）
    Missing(String),
    /// 配置的编辑器启动失败
    EditorFailed(String),
    /// 系统默认程序打开失败
    SystemFailed(String),
    /// 未知目标
    UnknownTarget(String),
}

impl OpenError {
    /// 面向用户的中文说明
    pub fn message(&self) -> String {
        match self {
            OpenError::Missing(d) => format!("目标文件不存在且无法创建：{d}"),
            OpenError::EditorFailed(d) => {
                format!("配置的编辑器启动失败：{d}（可在设置中改为留空以使用系统默认程序）")
            }
            OpenError::SystemFailed(d) => format!("系统默认程序打开失败：{d}"),
            OpenError::UnknownTarget(d) => format!("未知的打开目标：{d}"),
        }
    }
}

impl OpenTarget {
    /// 从 kebab-case 标识解析（前端传入的闭集标识）。
    ///
    /// 手写解析而非依赖 serde：错误信息可以点名合法取值，便于用户纠正。
    pub fn parse(value: &str) -> Result<Self, OpenError> {
        match value {
            "agents-md" => Ok(OpenTarget::AgentsMd),
            "context-md" => Ok(OpenTarget::ContextMd),
            "skills-root" => Ok(OpenTarget::SkillsRoot),
            other => Err(OpenError::UnknownTarget(format!(
                "{other}（可用值：agents-md / context-md / skills-root）"
            ))),
        }
    }

    /// 解析目标为绝对路径（**唯一**的路径来源，前端无法影响）
    pub fn resolve(self) -> PathBuf {
        match self {
            OpenTarget::AgentsMd => dshhome::agents_home_agents_md(),
            OpenTarget::ContextMd => dshhome::agents_home_context_md(),
            OpenTarget::SkillsRoot => dshhome::agents_skills_dir(),
        }
    }

    /// Agent 指令文件的初始模板（文件不存在时创建，避免默认程序报错）
    fn template(self) -> Option<&'static str> {
        match self {
            // 词表：与 CONTEXT.md 的既有约定一致（纯词表，不写实现细节）
            OpenTarget::ContextMd => Some(
                "# CONTEXT — 项目词表\n\n\
                 > 本文件是**词表（glossary）**，不是 spec，不记录实现细节。\n\n\
                 ## 核心名词\n\n\
                 - **术语**：一句话定义。\n",
            ),
            OpenTarget::AgentsMd => Some(
                "# AGENTS.md\n\n\
                 > 面向 agent 的全局指令。dsh 固定读取 `<dshHome>/AGENTS.md`，\n\
                 > 本文件是其共享真源。\n\n\
                 ## 通用规范\n\n\
                 - 在此写下你希望所有项目都遵守的规则。\n",
            ),
            // 目录不需要模板
            OpenTarget::SkillsRoot => None,
        }
    }

    /// 确保目标存在（文件不存在则按模板创建；目录不存在则创建）
    fn ensure_exists(self) -> Result<bool, OpenError> {
        let path = self.resolve();
        if path.exists() {
            return Ok(false);
        }
        match self.template() {
            Some(template) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        OpenError::Missing(format!("创建目录 {} 失败: {e}", parent.display()))
                    })?;
                }
                std::fs::write(&path, template).map_err(|e| {
                    OpenError::Missing(format!("创建 {} 失败: {e}", path.display()))
                })?;
                Ok(true)
            }
            None => {
                std::fs::create_dir_all(&path).map_err(|e| {
                    OpenError::Missing(format!("创建目录 {} 失败: {e}", path.display()))
                })?;
                Ok(true)
            }
        }
    }
}

/// 用**系统默认关联程序**打开（Windows：`ShellExecuteW` + `open` 动词）。
///
/// 刻意**不**用 `tauri_plugin_opener`：那会把整个 GUI DLL 栈链入库，使
/// `cargo test --lib` 的 unittest 二进制无法启动（见 `Cargo.toml` 的说明）。
/// `ShellExecuteW` 只依赖 shell32，语义相同且 `windows` crate 本就是依赖。
#[cfg(windows)]
fn open_with_system_default(path: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // ShellExecuteW 需要 UTF-16 且以 NUL 结尾
    let wide = |s: &str| -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    };
    let verb = wide("open");
    let file = wide(&path.to_string_lossy());

    // SAFETY: verb/file 均为本函数内构造的 NUL 结尾 UTF-16 缓冲，生命周期覆盖调用。
    // 返回值 >32 表示成功（ShellExecuteW 的错误码约定）。
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // HINSTANCE 是 `#[repr(transparent)]` 包裹的 `*mut c_void`；其地址值即错误码。
    // 约定：> 32 表示成功，<= 32 为失败错误码。
    let code = result.0 as isize;
    if code <= 32 {
        return Err(format!(
            "ShellExecuteW 返回错误码 {code}（可能没有可打开该类型文件的关联程序）"
        ));
    }
    Ok(())
}

/// 非 Windows：交给 `xdg-open`（开发环境用；产品目标平台是 Windows）
#[cfg(not(windows))]
fn open_with_system_default(path: &Path) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 xdg-open 失败: {e}"))
}

/// 用配置的编辑器启动（解析 `editor_command` 为「可执行文件 + 参数」）。
///
/// 支持 `code --wait`、`"C:\Program Files\...\Code.exe" --wait` 这类命令行：
/// 用简单的引号感知切分，避免引入 shell 词法库。
fn launch_editor(command: &str, path: &std::path::Path) -> Result<(), String> {
    let tokens = split_command_line(command);
    let Some((program, args)) = tokens.split_first() else {
        return Err("编辑器命令为空".to_string());
    };
    // CREATE_NO_WINDOW 不适用于 GUI 编辑器：需要它显示自己的窗口。
    // 这里用普通 spawn（不等待），编辑器自行管理生命周期。
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    cmd.arg(path);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 {program} 失败: {e}"))
}

/// 引号感知的命令行切分（支持 `"` 与 `'` 包裹，含空格路径）
fn split_command_line(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut has_content = false;
    for ch in input.trim().chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '"' | '\'' => {
                    quote = Some(ch);
                    has_content = true;
                }
                c if c.is_whitespace() => {
                    if has_content || !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                        has_content = false;
                    }
                }
                c => {
                    current.push(c);
                    has_content = true;
                }
            },
        }
    }
    if has_content || !current.is_empty() {
        out.push(current);
    }
    out
}

/// 打开一个闭集目标（Rust 侧执行，不受 ACL 限制）
pub fn open_target(
    target: OpenTarget,
    logger: &std::sync::Arc<crate::core::logging::Logger>,
) -> Result<OpenReport, OpenError> {
    let created = target.ensure_exists()?;
    let path = target.resolve();

    // ① 配置的编辑器优先
    let configured = AppConfig::load().editor_command;
    let configured = configured.trim();
    if !configured.is_empty() {
        match launch_editor(configured, &path) {
            Ok(()) => {
                logger.info(&format!(
                    "已用配置的编辑器打开 {}（命令 {configured}）",
                    path.display()
                ));
                return Ok(OpenReport {
                    path: path.display().to_string(),
                    via: "editor".to_string(),
                    created,
                });
            }
            Err(error) => {
                // ② 编辑器失败 → 退回系统默认，并记录原因（不静默失败）
                logger.warn(&format!(
                    "配置的编辑器启动失败，退回系统默认程序：{error}"
                ));
            }
        }
    }

    // ② 系统默认关联程序
    open_with_system_default(&path).map_err(OpenError::SystemFailed)?;
    logger.info(&format!("已用系统默认程序打开 {}", path.display()));
    Ok(OpenReport {
        path: path.display().to_string(),
        via: "system".to_string(),
        created,
    })
}

/// 用配置的编辑器 / 系统默认程序打开**某个受管技能文件**（ADR-0008）。
///
/// 与 [`open_target`] 不同，这里的路径来自前端 —— 因此必须按 ADR-0007 D10 的同一纪律
/// 校验：路径必须仍在受管技能根内且是真实存在的文件。校验不通过即拒绝，
/// **绝不**把前端传来的任意路径交给操作系统去打开。
pub fn open_skill_file(
    declared_path: &std::path::Path,
    logger: &std::sync::Arc<crate::core::logging::Logger>,
) -> Result<OpenReport, OpenError> {
    // 复用扫描模块的受管根归属判定（与启停/删除同一套信任边界）
    if crate::core::skill::scan::resolve_managed(declared_path).is_none() {
        return Err(OpenError::UnknownTarget(
            "该路径不在受管的用户级技能根内（或已不存在）".to_string(),
        ));
    }
    // G6（审计 SEC-05）：打开**已校验的规范化路径**。
    // 归属判定内部已 canonicalize；若此处仍用前端传入的原路径，
    // 判定与打开之间就存在符号链接替换窗口（TOCTOU）。
    let path = declared_path
        .canonicalize()
        .map_err(|e| OpenError::UnknownTarget(format!("无法解析技能文件路径: {e}")))?;

    let configured = AppConfig::load().editor_command;
    let configured = configured.trim();
    if !configured.is_empty() {
        match launch_editor(configured, &path) {
            Ok(()) => {
                logger.info(&format!("已用配置的编辑器打开技能文件 {}", path.display()));
                return Ok(OpenReport {
                    path: path.display().to_string(),
                    via: "editor".to_string(),
                    created: false,
                });
            }
            Err(error) => {
                logger.warn(&format!(
                    "配置的编辑器启动失败，退回系统默认程序：{error}"
                ));
            }
        }
    }

    open_with_system_default(&path).map_err(OpenError::SystemFailed)?;
    logger.info(&format!("已用系统默认程序打开技能文件 {}", path.display()));
    Ok(OpenReport {
        path: path.display().to_string(),
        via: "system".to_string(),
        created: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 命令行切分_裸命令() {
        assert_eq!(split_command_line("code"), vec!["code"]);
    }

    #[test]
    fn 命令行切分_带参数() {
        assert_eq!(split_command_line("code --wait"), vec!["code", "--wait"]);
    }

    #[test]
    fn 命令行切分_引号包裹的含空格路径() {
        assert_eq!(
            split_command_line("\"C:\\Program Files\\Microsoft VS Code\\Code.exe\" --wait"),
            vec!["C:\\Program Files\\Microsoft VS Code\\Code.exe", "--wait"]
        );
    }

    #[test]
    fn 命令行切分_单引号() {
        assert_eq!(
            split_command_line("'/opt/my editor/ed' -n"),
            vec!["/opt/my editor/ed", "-n"]
        );
    }

    #[test]
    fn 命令行切分_多余空白被忽略() {
        assert_eq!(
            split_command_line("   code    --wait   "),
            vec!["code", "--wait"]
        );
    }

    #[test]
    fn 命令行切分_空输入() {
        assert!(split_command_line("").is_empty());
        assert!(split_command_line("    ").is_empty());
    }

    #[test]
    fn 目标解析到_agents_home_之下() {
        let agents = dshhome::agents_home();
        assert!(OpenTarget::AgentsMd.resolve().starts_with(&agents));
        assert!(OpenTarget::ContextMd.resolve().starts_with(&agents));
        assert!(OpenTarget::SkillsRoot.resolve().starts_with(&agents));
    }

    #[test]
    fn agents_md_与_context_md_目标不同() {
        assert_ne!(
            OpenTarget::AgentsMd.resolve(),
            OpenTarget::ContextMd.resolve()
        );
    }
}
