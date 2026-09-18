//! 无 GUI CLI（同一二进制）：`dsh-launcher plugin ...` / `skill ...` / `mcp ...`
//!
//! 与 Tauri IPC **共用同一 core**，因此 CLI 与 GUI 的状态机、幂等、回滚语义完全一致；
//! CLI 存在的意义是让端到端验收可脚本化（无 GUI 环境也能跑集成测试）。
//!
//! 退出码见 ADR-0005 / ADR-0006（**零新增数值**）：0 成功/空操作；2 非法转换；
//! 3 未找到；4 忙；5 校验失败；6 能力缺失；7 受管区块冲突；8 dsh 未安装。

use crate::core::config::AppConfig;
use crate::core::dshhome::MANAGED_PROFILE;
use crate::core::logging::Logger;
use crate::core::mcp::{self, McpAddSpec};
use crate::core::mcp::state::McpAction;
use crate::core::plugin::{self, state::PluginError};
use crate::core::plugin::spec::Origin;
use crate::core::process::ProcessManager;
use crate::core::skill;
use serde::Serialize;
use std::sync::Arc;

/// 是否应进入 CLI 模式（第一个参数是已知子命令/帮助）
pub fn is_cli_invocation(args: &[String]) -> bool {
    matches!(
        args.first().map(String::as_str),
        Some("plugin") | Some("skill") | Some("mcp") | Some("--help") | Some("-h") | Some("--version") | Some("-V")
    )
}

/// CLI 帮助
const HELP: &str = "\
dsh-launcher — deepseek-harness 启动器与管理器

用法：
  dsh-launcher                              启动图形界面
  dsh-launcher plugin list [--json]
  dsh-launcher plugin install <spec> [--origin upstream|in-house|unknown]
  dsh-launcher plugin enable <package>
  dsh-launcher plugin disable <package>
  dsh-launcher plugin uninstall <package>
  dsh-launcher plugin sync [--check] [--package <p>]
  dsh-launcher plugin repair [--package <p>]
    dsh-launcher skill status [--json]
  dsh-launcher skill apply [--mode auto|link|config] [--resource skills|agents-md|context-md]
  dsh-launcher skill repair-links [--json]
  dsh-launcher skill migrate [--dry-run]
  dsh-launcher mcp list [--json]
  dsh-launcher mcp add --server-name <n> --transport <stdio|streamable-http> [选项]
  dsh-launcher mcp remove  <serverName> [--json]
  dsh-launcher mcp enable  <serverName> [--json]
  dsh-launcher mcp disable <serverName> [--json]

mcp add 选项（除 --raw-config 外均为官方字段名）：
  --id <rowId>                       行 id（默认 mcp-<serverName>）
  --start-disabled                   声明为 disabled（默认 enabled）
  stdio：--command <c> [--arg <a>]... [--env <K=V>]... [--cwd <d>]
  streamable-http：--url <u> [--header <K=V>]...
  --tool-call-timeout-ms <n>
  --reconnect-enabled <true|false> --reconnect-initial-delay-ms <n>
  --reconnect-max-delay-ms <n> --reconnect-max-attempts <n>
  --raw-config <file>                官方 config 体的原始 YAML 片段（与结构化字段互斥）

说明：
  插件按 ID（包名）独立管理生命周期；启停写 profile 的受管 patch 区块并由 dsh 热重载，
  装卸走官方 `dsh plugin --profile web ...` 通道并在需要时自动重启 dsh。
  技能共享以官方 agentsHome 根为真源（技能 ~/.agents/skills；指令 ~/.agents/AGENTS.md）。
  MCP server 写 $DSH_HOME/cordis.patch.yml 的受管 MCP 区块；启停走行级 disabled 定向
  覆盖，三类变更**都不重启 dsh**（受管 profile web 为 patchReload=live）。
";

/// CLI 入口
pub fn run(args: &[String]) -> i32 {
    attach_parent_console();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{HELP}");
        return 0;
    }
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("dsh-launcher {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }

    let logger = Arc::new(Logger::init());
    // 陈旧 dsh shim 自愈（与 GUI 启动路径一致）：CLI 用户（`dsh-launcher plugin ...`）
    // 同样可能受 PATH 首位陈旧 GitHub shim 遮蔽（`dsh --dump-config` 报「系统找不到
    // 指定的路径」）。先清掉指向已失效目录的自家 shim，仍有效的不碰。
    crate::core::github::remove_stale_github_shims(&logger);
    let process = Arc::new(ProcessManager::new(Arc::clone(&logger)));
    let port = AppConfig::load().port;
    if port != 0 && crate::core::port::is_port_in_use(port) {
        process.adopt_running(port);
    }

    let Some(group) = args.first() else {
        print!("{HELP}");
        return 0;
    };
    match group.as_str() {
        "plugin" => run_plugin(&args[1..], &logger, &process),
        "skill" => run_skill(&args[1..], &logger),
        "mcp" => run_mcp(&args[1..], &logger),
        other => {
            eprintln!("未知子命令: {other}\n");
            print!("{HELP}");
            2
        }
    }
}

fn run_plugin(args: &[String], logger: &Arc<Logger>, process: &Arc<ProcessManager>) -> i32 {
    let json = has_flag(args, "--json");
    let Some(action) = args.first().map(String::as_str) else {
        eprintln!("plugin 需要动作：list|install|enable|disable|uninstall|sync|repair");
        return 2;
    };
    match action {
        "list" => match plugin::list(MANAGED_PROFILE, logger) {
            Ok(list) => {
                if json {
                    return print_json(&list);
                } else {
                    if let Some(reason) = &list.degraded_reason {
                        eprintln!("注意：{reason}");
                    }
                    if list.plugins.is_empty() {
                        println!("（无受管插件）");
                    }
                    for item in &list.plugins {
                        println!(
                            "{:<44} {:<11} {:<10} {}{}",
                            item.package,
                            item.state.as_str(),
                            item.origin_str(),
                            item.version.clone().unwrap_or_else(|| "-".to_string()),
                            if item.protected { "  [受保护]" } else { "" }
                        );
                    }
                }
                0
            }
            Err(error) => report(error),
        },
        "install" => {
            let Some(spec) = positional(args, 1) else {
                eprintln!("用法: dsh-launcher plugin install <spec> [--origin ...]");
                return 2;
            };
            let origin = match flag_value(args, "--origin").as_deref() {
                None | Some("") => None,
                Some("upstream") => Some(Origin::Upstream),
                Some("in-house") => Some(Origin::InHouse),
                Some("unknown") => Some(Origin::Unknown),
                Some(other) => {
                    eprintln!("未知来源: {other}");
                    return 2;
                }
            };
            match plugin::install(MANAGED_PROFILE, &spec, origin, logger, Some(process)) {
                Ok(result) => {
                    if json {
                        return print_json(&result);
                    } else {
                        println!("{}", result.message);
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        "enable" | "disable" => {
            let Some(package) = positional(args, 1) else {
                eprintln!("用法: dsh-launcher plugin {action} <package>");
                return 2;
            };
            let enabled = action == "enable";
            match plugin::set_state(MANAGED_PROFILE, &package, enabled, logger) {
                Ok(result) => {
                    if json {
                        return print_json(&result);
                    } else {
                        println!("{}", result.message);
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        "uninstall" => {
            let Some(package) = positional(args, 1) else {
                eprintln!("用法: dsh-launcher plugin uninstall <package>");
                return 2;
            };
            match plugin::uninstall(MANAGED_PROFILE, &package, logger, Some(process)) {
                Ok(result) => {
                    if json {
                        return print_json(&result);
                    } else {
                        println!("{}", result.message);
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        "sync" => {
            let apply = !has_flag(args, "--check");
            let only = flag_value(args, "--package");
            match plugin::sync(MANAGED_PROFILE, apply, only.as_deref(), logger, Some(process)) {
                Ok(report) => {
                    if json {
                        return print_json(&report);
                    } else {
                        for item in &report.items {
                            println!(
                                "{:<44} {:<8} {} → {}  {}",
                                item.package,
                                item.result,
                                item.from.clone().unwrap_or_else(|| "-".to_string()),
                                item.to.clone().unwrap_or_else(|| "-".to_string()),
                                item.message
                            );
                        }
                        if !report.applied {
                            println!("（仅检查；加 --check 之外不带该参数即执行同步）");
                        }
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        "repair" => {
            let only = flag_value(args, "--package");
            match plugin::repair(MANAGED_PROFILE, only.as_deref(), logger, Some(process)) {
                Ok(result) => {
                    if json {
                        return print_json(&result);
                    } else {
                        println!("{}", result.message);
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        other => {
            eprintln!("未知 plugin 动作: {other}");
            2
        }
    }
}

fn run_skill(args: &[String], logger: &Arc<Logger>) -> i32 {
    let json = has_flag(args, "--json");
    let Some(action) = args.first().map(String::as_str) else {
        eprintln!("skill 需要动作：status|apply|migrate");
        return 2;
    };
    match action {
        "status" => {
            let status = skill::status();
            if json {
                return print_json(&status);
            } else {
                println!("共享真源: {}", status.canonical_root);
                println!(
                    "DSH_HOME: {}    agentsHome: {}",
                    status.dsh_home, status.agents_home
                );
                println!(
                    "链接能力: {}    生效模式: {}    偏好: {}    技能数: {}",
                    if status.link_capable { "可用" } else { "不可用" },
                    status.active_mode,
                    status.preferred_mode,
                    status.skill_count
                );
                for item in &status.resources {
                    println!(
                        "  {:<12} {:<9} {}",
                        item.resource,
                        format!("{:?}", item.state).to_lowercase(),
                        item.detail
                    );
                }
            }
            0
        }
        "apply" => {
            let mode = flag_value(args, "--mode").unwrap_or_else(|| "auto".to_string());
            let resource = flag_value(args, "--resource");
            match skill::apply(MANAGED_PROFILE, &mode, resource.as_deref(), logger) {
                Ok(report) => {
                    if json {
                        return print_json(&report);
                    } else {
                        println!("{}", report.message);
                        for item in &report.resources {
                            println!(
                                "  {:<12} {:<9} {}",
                                item.resource,
                                format!("{:?}", item.state).to_lowercase(),
                                item.detail
                            );
                        }
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        "migrate" => {
            let dry_run = has_flag(args, "--dry-run");
            match skill::migrate(dry_run, logger) {
                Ok(report) => {
                    if json {
                        return print_json(&report);
                    } else {
                        println!(
                            "{}",
                            if dry_run {
                                "迁移预演（未落盘）："
                            } else {
                                "迁移结果："
                            }
                        );
                        for item in &report.actions {
                            println!("  {:<12} {:<12} {}", item.resource, item.action, item.detail);
                        }
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        "repair-links" => {
            // ADR-0006 D17 迁移动作：修复指令链接 + 清理启动器自己留下的断链
            match skill::repair_links(MANAGED_PROFILE, logger) {
                Ok(report) => {
                    if json {
                        return print_json(&report);
                    }
                    println!("链接修复结果：");
                    for item in &report.actions {
                        println!("  {:<12} {:<20} {}", item.resource, item.action, item.detail);
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        other => {
            eprintln!("未知 skill 动作: {other}");
            2
        }
    }
}

// ============================ mcp（ADR-0006） ============================

/// 单个带值选项的游标式解析（`--flag` 与 `--flag value` 都支持）。
struct ArgCursor<'a> {
    args: &'a [String],
    index: usize,
}

impl<'a> ArgCursor<'a> {
    fn new(args: &'a [String]) -> Self {
        Self { args, index: 0 }
    }

    fn next(&mut self) -> Option<&'a str> {
        let value = self.args.get(self.index).map(String::as_str);
        self.index += 1;
        value
    }

    fn take_value(&mut self, flag: &str) -> Result<String, PluginError> {
        match self.next() {
            Some(value) if !value.starts_with("--") => Ok(value.to_string()),
            // `--arg --flag` 是合法用法：MCP 子进程自身的选项常以 `-`/`--` 开头，
            // 而 `--arg` / `--env` / `--header` 的值本来就可以是任意字符串。
            // 因此对这几个"任意值"选项放行 `-` 前缀，其余选项保持严格以避免误吞。
            Some(value) if ARBITRARY_VALUE_FLAGS.contains(&flag) && value != "--" => {
                Ok(value.to_string())
            }
            _ => Err(PluginError::illegal(format!("{flag} 需要一个值"))),
        }
    }
}

/// 值可以是任意字符串（含 `-` 前缀）的选项
const ARBITRARY_VALUE_FLAGS: [&str; 3] = ["--arg", "--env", "--header"];

/// 解析 `K=V` / `K: V` 形态的键值项（`--env` / `--header`）。
fn parse_pair(flag: &str, raw: &str) -> Result<(String, String), PluginError> {
    let split = raw
        .split_once('=')
        .or_else(|| raw.split_once(':'));
    match split {
        Some((key, value)) if !key.trim().is_empty() => {
            Ok((key.trim().to_string(), value.trim().to_string()))
        }
        _ => Err(PluginError::illegal(format!(
            "{flag} 需要 K=V 形态（如 --env FOO=bar）"
        ))),
    }
}

/// 解析 `--reconnect-enabled` 的布尔值（只接受官方字面量 true/false）。
fn parse_bool(flag: &str, raw: &str) -> Result<bool, PluginError> {
    match raw.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(PluginError::illegal(format!(
            "{flag} 只接受 true/false（收到 {other:?}）"
        ))),
    }
}

/// 解析数值选项。
fn parse_u64(flag: &str, raw: &str) -> Result<u64, PluginError> {
    raw.trim()
        .parse::<u64>()
        .map_err(|_| PluginError::illegal(format!("{flag} 需要非负整数（收到 {raw:?}）")))
}

/// CLI `mcp` 子命令入口。
fn run_mcp(args: &[String], logger: &Arc<Logger>) -> i32 {
    let json = has_flag(args, "--json");
    let Some(action) = args.first().map(String::as_str) else {
        eprintln!("mcp 需要动作：list|add|remove|enable|disable");
        return 2;
    };
    match action {
        "list" => match mcp::list(logger) {
            Ok(result) => {
                if json {
                    return print_json(&result);
                }
                println!(
                    "profile {} · 生效方式 {} · 前置 {}",
                    result.profile,
                    match result.reload {
                        mcp::ReloadMode::Live => "live（改配置就地热重载，不重启 dsh）",
                        mcp::ReloadMode::RequiresRestart => "requires-restart（需重启 dsh）",
                    },
                    if result.prereq.installed {
                        "已就绪"
                    } else {
                        "缺失（add 不可用，请到插件页安装）"
                    }
                );
                if result.servers.is_empty() {
                    println!("（合成树中没有 MCP server 行）");
                }
                for server in &result.servers {
                    let marks: Vec<&str> = server
                        .marks
                        .iter()
                        .map(|mark| match mark {
                            mcp::state::McpMark::Expression => "表达式",
                            mcp::state::McpMark::Conflict => "冲突",
                            mcp::state::McpMark::Dangerous => "危险",
                        })
                        .collect();
                    println!(
                        "{:<20} {:<16} {:<9} {:<9} {}{}{}",
                        server.server_name,
                        server
                            .transport
                            .map(|transport| match transport {
                                mcp::state::McpTransportView::Stdio => "stdio",
                                mcp::state::McpTransportView::StreamableHttp => "streamable-http",
                            })
                            .unwrap_or("-"),
                        server.state.as_str(),
                        match server.origin {
                            mcp::state::McpOrigin::Managed => "managed",
                            mcp::state::McpOrigin::External => "external",
                        },
                        server.summary,
                        if marks.is_empty() { "" } else { "  [" },
                        if marks.is_empty() {
                            String::new()
                        } else {
                            format!("{}]", marks.join("/"))
                        }
                    );
                    println!("  {:<17} 来源层 {}", "", server.layer);
                }
                0
            }
            Err(error) => report(error),
        },
        "add" => match build_add_spec(&args[1..]) {
            Ok(spec) => match mcp::add(&spec, logger) {
                Ok(result) => {
                    if json {
                        return print_json(&result);
                    }
                    println!("{}", result.message);
                    0
                }
                Err(error) => report(error),
            },
            Err(error) => report(error),
        },
        "remove" => {
            let Some(server_name) = positional(args, 1) else {
                eprintln!("用法: dsh-launcher mcp remove <serverName> [--json]");
                return 2;
            };
            match mcp::remove(&server_name, logger) {
                Ok(result) => {
                    if json {
                        return print_json(&result);
                    }
                    println!("{}", result.message);
                    0
                }
                Err(error) => report(error),
            }
        }
        "enable" | "disable" => {
            let Some(server_name) = positional(args, 1) else {
                eprintln!("用法: dsh-launcher mcp {action} <serverName> [--json]");
                return 2;
            };
            let enabled = action == "enable";
            match mcp::set_state(
                &server_name,
                if enabled {
                    McpAction::Enable
                } else {
                    McpAction::Disable
                },
                logger,
            ) {
                Ok(result) => {
                    if json {
                        return print_json(&result);
                    }
                    println!("{}", result.message);
                    0
                }
                Err(error) => report(error),
            }
        }
        other => {
            eprintln!("未知 mcp 动作: {other}（可选 list|add|remove|enable|disable）");
            2
        }
    }
}

/// 解析 `mcp add` 的选项为结构化入参（**全部为官方字段名**）。
fn build_add_spec(args: &[String]) -> Result<McpAddSpec, PluginError> {
    let mut spec = McpAddSpec {
        server_name: String::new(),
        transport: String::new(),
        ..Default::default()
    };
    let mut cursor = ArgCursor::new(args);
    while let Some(arg) = cursor.next() {
        // 支持 `--flag=value` 形态（对以 `-` 开头的值最稳妥）
        let (arg, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (arg, None),
        };
        let take = |cursor: &mut ArgCursor| match &inline {
            Some(value) => Ok(value.clone()),
            None => cursor.take_value(arg),
        };
        match arg {
            "--server-name" => spec.server_name = take(&mut cursor)?,
            "--transport" => spec.transport = take(&mut cursor)?,
            "--id" => spec.row_id = Some(take(&mut cursor)?),
            "--start-disabled" => spec.start_disabled = true,
            "--command" => spec.command = Some(take(&mut cursor)?),
            "--arg" => spec.args.push(take(&mut cursor)?),
            "--env" => {
                let raw = take(&mut cursor)?;
                spec.env.push(parse_pair(arg, &raw)?);
            }
            "--cwd" => spec.cwd = Some(take(&mut cursor)?),
            "--url" => spec.url = Some(take(&mut cursor)?),
            "--header" => {
                let raw = take(&mut cursor)?;
                spec.headers.push(parse_pair(arg, &raw)?);
            }
            "--tool-call-timeout-ms" => {
                let raw = take(&mut cursor)?;
                spec.tool_call_timeout_ms = Some(parse_u64(arg, &raw)?);
            }
            "--reconnect-enabled" => {
                let raw = take(&mut cursor)?;
                spec.reconnect_enabled = Some(parse_bool(arg, &raw)?);
            }
            "--reconnect-initial-delay-ms" => {
                let raw = take(&mut cursor)?;
                spec.reconnect_initial_delay_ms = Some(parse_u64(arg, &raw)?);
            }
            "--reconnect-max-delay-ms" => {
                let raw = take(&mut cursor)?;
                spec.reconnect_max_delay_ms = Some(parse_u64(arg, &raw)?);
            }
            "--reconnect-max-attempts" => {
                let raw = take(&mut cursor)?;
                spec.reconnect_max_attempts = Some(parse_u64(arg, &raw)?);
            }
            "--raw-config" => {
                let path = take(&mut cursor)?;
                let raw = std::fs::read_to_string(&path).map_err(|e| {
                    PluginError::illegal(format!("读取 --raw-config 文件 {path} 失败: {e}"))
                })?;
                spec.raw_config = Some(raw);
            }
            "--json" => {}
            other => {
                return Err(PluginError::illegal(format!("未知选项: {other}")));
            }
        }
    }
    if spec.server_name.trim().is_empty() {
        return Err(PluginError::illegal(
            "必须提供 --server-name（面向模型的稳定身份，官方要求 ^[A-Za-z0-9_-]{1,32}$）",
        ));
    }
    if spec.transport.trim().is_empty() {
        return Err(PluginError::illegal(
            "必须提供 --transport <stdio|streamable-http>",
        ));
    }
    Ok(spec)
}

fn report(error: PluginError) -> i32 {
    eprintln!("错误: {}", error.message);
    error.kind.exit_code()
}

fn print_json<T: Serialize>(value: &T) -> i32 {
    match serde_json::to_string_pretty(value) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(error) => {
            eprintln!("序列化输出失败: {error}");
            1
        }
    }
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.get(index + 1).cloned()
}

/// 取第 n 个非选项参数（跳过 `--flag` 与其值）。
fn positional(args: &[String], n: usize) -> Option<String> {
    let mut values: Vec<&String> = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg.starts_with("--") {
            // 只有带值的已知选项才跳过下一个参数
            if matches!(
                arg.as_str(),
                "--origin" | "--package" | "--mode" | "--resource"
            ) {
                skip_next = true;
            }
            continue;
        }
        values.push(arg);
    }
    values.get(n).map(|value| (*value).clone())
}

/// release 构建是 windows 子系统（无控制台）：附加到父进程控制台后 CLI 输出才可见。
///
/// 关键点：`AttachConsole` 会改写本进程的标准句柄，因此必须**先**保存启动时继承的
/// stdout/stderr（管道或重定向文件），附加之后再恢复；两者都无效（终端直跑）时才用
/// `CONOUT$` 兜底。这样三种调用方式（终端直跑 / `>` 重定向 / `|` 管道）都能拿到输出。
fn attach_parent_console() {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Console::{
            AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
            STD_OUTPUT_HANDLE,
        };
        // SAFETY: 只影响本进程的控制台附加与标准句柄；失败时保持原状
        unsafe {
            let saved = [
                (STD_OUTPUT_HANDLE, GetStdHandle(STD_OUTPUT_HANDLE).ok()),
                (STD_ERROR_HANDLE, GetStdHandle(STD_ERROR_HANDLE).ok()),
            ];
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            for (id, handle) in saved {
                match handle {
                    Some(h) if !h.0.is_null() => {
                        let _ = SetStdHandle(id, h);
                    }
                    _ => {
                        if let Ok(file) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                            let _ = SetStdHandle(id, HANDLE(file.as_raw_handle() as *mut _));
                            // 句柄需与进程同生命周期：泄漏这一个 File 是有意的
                            std::mem::forget(file);
                        }
                    }
                }
            }
        }
    }
    #[cfg(not(all(windows, not(debug_assertions))))]
    {
        // debug 构建带控制台；非 Windows 平台无需处理
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn test_is_cli_invocation() {
        assert!(is_cli_invocation(&args(&["plugin", "list"])));
        assert!(is_cli_invocation(&args(&["skill", "status"])));
        assert!(is_cli_invocation(&args(&["--help"])));
        assert!(!is_cli_invocation(&args(&["--web-gui"])));
        assert!(!is_cli_invocation(&[]));
    }

    #[test]
    fn test_flag_helpers() {
        let list = args(&["install", "pkg", "--origin", "upstream", "--json"]);
        assert_eq!(positional(&list, 1).as_deref(), Some("pkg"));
        assert_eq!(flag_value(&list, "--origin").as_deref(), Some("upstream"));
        assert!(has_flag(&list, "--json"));
        assert_eq!(flag_value(&list, "--package"), None);
    }

    #[test]
    fn test_positional_skips_option_values() {
        let list = args(&["sync", "--package", "dshmarket"]);
        assert_eq!(positional(&list, 0).as_deref(), Some("sync"));
        assert_eq!(positional(&list, 1), None);
    }
}
