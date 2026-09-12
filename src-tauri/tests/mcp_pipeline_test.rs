//! MCP 管理集成测试（端到端核心链路；不依赖真实 dsh / 不改动真实 `~/.dsh`）
//!
//! 隔离方式：
//! - `DSH_HOME` 指向临时目录 → 受管区块文件是 `<tmp>/cordis.patch.yml`；
//! - `DSH_AGENTS_HOME` 指向临时目录（前置探测不受本机 `~/.agents` 影响）；
//! - `LOCALAPPDATA` 指向临时目录 → `%LOCALAPPDATA%\dsh-launcher\backups\mcp\...`
//!   与 GitHub 安装目录同根，且 `profile::install_dir()` 返回 None → 回落 PATH；
//! - PATH 前置一个假 `dsh.cmd`（PowerShell 实现），它**只做 patch 合成**：
//!   解析 `cordis.patch.yml` 的 `- insert:` 声明并叠加行级 `disabled` 定向覆盖，
//!   打印与官方 `renderConfigDump` 同形的 dump（含 `# == <绝对路径>` 段标签）。
//!
//! 覆盖 ADR-0006 §Testing 3.2 的六类场景。
//!
//! 注：环境变量是进程级的，故所有用例共用一把互斥锁串行执行。

use dsh_launcher_lib::core::mcp::state::{McpAction, McpMark, McpOrigin, McpState};
use dsh_launcher_lib::core::mcp::{self, McpAddSpec};
use dsh_launcher_lib::core::plugin::state::PluginErrorKind;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// 环境变量互斥（进程级共享状态，用例必须串行）
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 假 `dsh` 的 PowerShell 实现：从 patch 文件合成 dump。
///
/// 官方语义对应（`$DSH_SRC`）：
/// - patch 层的声明必须包在 `- insert:` 里（顶层裸 `- id:` 不产生行）；
/// - 行级 `disabled` 是覆盖：后面的定向条目（与更后层）胜；
/// - 段标签 `# == <绝对路径>` 给出"这一行来自哪个文件"。
///
/// **本脚本必须保持纯 ASCII**：Windows PowerShell 5.1 按 ANSI 代码页读取
/// 无 BOM 的 `.ps1`，中文注释会被误解码并破坏脚本解析。
const FAKE_DSH_PS1: &str = r#"
$ErrorActionPreference = 'Stop'
$home2 = $env:DSH_HOME
$path = Join-Path $home2 'cordis.patch.yml'
$lines = @()
if (Test-Path $path) { $lines = @(Get-Content -LiteralPath $path) }

# Segment 1: declarations = `- id:` inside `- insert:` (indent >= 4)
# Segment 2: directives = column-0 `- id:` followed by an indented `disabled:`
# Both `disabled:` forms share 2-space indent, so column 0 is the discriminator.
$declares = New-Object System.Collections.ArrayList
$applied = @{}
$inInsert = $false
$cur = $null
$pending = $null
foreach ($line in $lines) {
    if ($line -match '^#') { continue }
    if ($line -match '^-\s*insert:\s*$') { $inInsert = $true; $cur = $null; $pending = $null; continue }
    if ($line -match '^(\s*)-\s*id:\s*(.+?)\s*$') {
        $indent = $Matches[1].Length
        $id = $Matches[2].Trim().Trim("'").Trim('"')
        # Inside `- insert:` the list items are indented (>= 4); the list itself is
        # closed by ANY shallower `- id:` line, whether it is a nested list item or a
        # top-level directive. Both cases must leave insert mode.
        if ($inInsert -and $indent -ge 4) {
            $node = New-Object System.Collections.ArrayList
            [void]$declares.Add(@{ id = $id; node = $node })
            $cur = @{ id = $id; node = $node }
            continue
        }
        $inInsert = $false
        $cur = $null
        $pending = $id
        $applied[$id] = $null
        continue
    }
    if ($inInsert -and $null -ne $cur) {
        $t = $line.Trim()
        if ($t -match '^name:' -or $t -match '^config:\s*$') { continue }
        # A declaration may carry its own row-level `disabled:` (official form 1).
        if ($t -match '^disabled:\s*(true|false)\s*$') {
            $applied[$cur.id] = $Matches[1]
            continue
        }
        if ($t.Length -gt 0) { [void]$cur.node.Add($line) }
        continue
    }
    if (-not $inInsert -and $null -ne $pending -and $line -match '^\s+disabled:\s*(.+?)\s*$') {
        # Official dumps print the effective value verbatim: the literal true/false for
        # a directive override, or the `!!js ...` expression of a declaration-controlled row.
        $applied[$pending] = $Matches[1]
    }
}

# A column-0 `- id:` outside the insert list is a directive and resets insert mode.
# (handled above: the `$indent -eq 0` branch clears $inInsert)

Write-Output ('# == ' + $path)
foreach ($d in $declares) {
    $body = @()
    foreach ($raw in $d.node) { $body += $raw }
    # Re-indent the config body to a 2-space base (matches official yaml.dump)
    $base = 99
    foreach ($b in $body) { if ($b.Trim().Length -gt 0) { $ind = $b.Length - $b.TrimStart().Length; if ($ind -lt $base) { $base = $ind } } }
    if ($base -eq 99) { $base = 0 }
    Write-Output ('- id: ' + $d.id)
    Write-Output "  name: '@deepseek-ai/dsh-mcp-client'"
    # Official dumps print the effective `disabled` verbatim: a literal true/false, or
    # the `!!js ...` expression for a row whose `disabled` an expression controls.
    $appliedValue = $null
    if ($applied.ContainsKey($d.id)) { $appliedValue = $applied[$d.id] }
    if ($null -ne $appliedValue -and ($appliedValue -match '^(true|false)$' -or $appliedValue -match '!!js')) {
        Write-Output ('  disabled: ' + $appliedValue)
    }
    Write-Output '  config:'
    foreach ($b in $body) {
        if ($b.Trim().Length -eq 0) { continue }
        $shifted = $b.Substring([Math]::Min($base, $b.Length))
        Write-Output ('    ' + $shifted)
    }
}
"#;

/// 测试工作区：临时 DSH_HOME / DSH_AGENTS_HOME / LOCALAPPDATA + 假 dsh
struct Fixture {
    root: PathBuf,
    dsh_home: PathBuf,
    local_appdata: PathBuf,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!(
            "dsh-launcher-mcp-it-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let dsh_home = root.join("dsh-home");
        let local_appdata = root.join("localappdata");
        let agents_home = root.join("agents-home");
        let bin = root.join("bin");
        for dir in [&dsh_home, &local_appdata, &agents_home, &bin] {
            std::fs::create_dir_all(dir).unwrap();
        }
        // profile 目录 + 一个 `live` 的 profile（决定 list.reload）
        let profile = dsh_home.join("profiles").join("web");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            profile.join("package.json"),
            r#"{"name":"dsh-profile-web","dependencies":{},"dsh":{"profile":{"bundles":[],"patchReload":"live"}}}"#,
        )
        .unwrap();
        std::fs::write(
            profile.join("cordis.patch.yml"),
            "# profile patch\n[]\n",
        )
        .unwrap();
        // 前置包（可解析性探测只要求目录 + package.json）
        let pkg = dsh_home
            .join("profiles")
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh-mcp-client");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("package.json"), r#"{"name":"@deepseek-ai/dsh-mcp-client"}"#)
            .unwrap();
        // 假 dsh
        let script = bin.join("fake-dsh.ps1");
        std::fs::write(&script, FAKE_DSH_PS1).unwrap();
        std::fs::write(
            bin.join("dsh.cmd"),
            format!(
                "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"{}\" %*\r\n",
                script.display()
            ),
        )
        .unwrap();

        std::env::set_var("DSH_HOME", &dsh_home);
        std::env::set_var("DSH_AGENTS_HOME", &agents_home);
        std::env::set_var("LOCALAPPDATA", &local_appdata);
        let path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{};{}", bin.display(), path));

        Self {
            root,
            dsh_home,
            local_appdata,
            _guard: guard,
        }
    }

    fn patch_path(&self) -> PathBuf {
        self.dsh_home.join("cordis.patch.yml")
    }

    fn write_user_patch(&self, content: &str) {
        std::fs::write(self.patch_path(), content).unwrap();
    }

    fn patch_bytes(&self) -> Vec<u8> {
        std::fs::read(self.patch_path()).unwrap_or_default()
    }

    fn logger(&self) -> Arc<dsh_launcher_lib::core::logging::Logger> {
        Arc::new(dsh_launcher_lib::core::logging::Logger::init())
    }

    fn list(&self) -> mcp::McpListResult {
        mcp::list(&self.logger()).expect("list 必须成功")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(path) = std::env::var_os("PATH") {
            let text = path.to_string_lossy().to_string();
            let bin = self.root.join("bin").display().to_string();
            let cleaned = text
                .split(';')
                .filter(|item| !item.is_empty() && *item != bin)
                .collect::<Vec<_>>()
                .join(";");
            std::env::set_var("PATH", cleaned);
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 用户手写的机器级 patch（4 条 MCP 声明，逐字节不得被改写）
const USER_PATCH: &str = concat!(
    "# ~/.dsh/cordis.patch.yml — 用户手写机器级 patch 层\n",
    "# 用户自己的注释\n",
    "\n",
    "- insert:\n",
    "    # --- Streamable HTTP 型 ---\n",
    "    - id: mcp-github\n",
    "      name: '@deepseek-ai/dsh-mcp-client'\n",
    "      config:\n",
    "        serverName: github\n",
    "        transport: streamable-http\n",
    "        url: https://api.githubcopilot.com/mcp/\n",
    "        headers:\n",
    "          Authorization: !!js >-\n",
    "            (() => 'Bearer x')()\n",
    "        toolCallTimeoutMs: 60000\n",
    "        failOnStartupError: false\n",
    "\n",
    "    - id: mcp-shadcn\n",
    "      name: '@deepseek-ai/dsh-mcp-client'\n",
    "      config:\n",
    "        serverName: shadcn\n",
    "        transport: stdio\n",
    "        command: shadcn\n",
    "        args: [mcp]\n",
    "        cwd: !!js process.cwd()\n",
);

fn stdio_spec(server_name: &str, command: &str) -> McpAddSpec {
    McpAddSpec {
        server_name: server_name.to_string(),
        transport: "stdio".to_string(),
        command: Some(command.to_string()),
        args: vec!["--flag".to_string()],
        ..Default::default()
    }
}

// ==================== 场景 1：外部行只读发现 ====================

#[test]
fn list_reports_external_user_rows_with_layer_and_marks() {
    let fixture = Fixture::new("list-external");
    fixture.write_user_patch(USER_PATCH);

    let result = fixture.list();
    assert_eq!(result.profile, "web");
    // `patchReload: live` → 不重启
    assert_eq!(result.reload, mcp::ReloadMode::Live);
    assert!(result.prereq.installed, "前置包已就位");

    let names: Vec<&str> = result
        .servers
        .iter()
        .map(|server| server.server_name.as_str())
        .collect();
    assert_eq!(names, vec!["github", "shadcn"]);
    for server in &result.servers {
        // 用户手写声明 → external，来源层 = 机器级 patch 文件
        assert_eq!(server.origin, McpOrigin::External);
        assert_eq!(server.state, McpState::Enabled);
        assert_eq!(server.layer, fixture.patch_path().display().to_string());
        assert!(server.marks.is_empty(), "{:?}", server.marks);
        assert_eq!(server.disabled, None);
    }
    let github = &result.servers[0];
    assert_eq!(github.row_id, "mcp-github");
    assert_eq!(github.summary, "https://api.githubcopilot.com/mcp/");
    let shadcn = &result.servers[1];
    // USER_PATCH 里 shadcn 的 args 是流式序列 `[mcp]`
    assert_eq!(shadcn.summary, "shadcn mcp");
}

// ==================== 场景 2：add（stdio + streamable-http）====================

#[test]
fn add_writes_declaration_and_directive_without_touching_outside_bytes() {
    let fixture = Fixture::new("add");
    fixture.write_user_patch(USER_PATCH);
    let before = fixture.patch_bytes();

    // stdio
    let result = mcp::add(&stdio_spec("playwright", "playwright-mcp"), &fixture.logger()).unwrap();
    assert_eq!(result.status, "changed");
    assert!(!result.restarted, "MCP 变更不重启 dsh");
    assert_eq!(result.server_name.as_deref(), Some("playwright"));

    // streamable-http
    let mut http = McpAddSpec {
        server_name: "remote".to_string(),
        transport: "streamable-http".to_string(),
        url: Some("https://example.test/mcp".to_string()),
        tool_call_timeout_ms: Some(15000),
        reconnect_max_attempts: Some(3),
        ..Default::default()
    };
    http.headers
        .push(("X-Test".to_string(), "1".to_string()));
    let result = mcp::add(&http, &fixture.logger()).unwrap();
    assert_eq!(result.status, "changed");

    let content = std::fs::read_to_string(fixture.patch_path()).unwrap();
    // 用户手写段逐字节保留在文件开头
    assert!(content.starts_with(USER_PATCH), "块外字节必须不变:\n{content}");
    assert!(content.starts_with(&String::from_utf8_lossy(&before)[..]));
    // 受管区块存在，且声明包在 insert 里
    assert!(content.contains("# >>> dsh-launcher mcp v1"));
    assert!(content.contains("# <<< dsh-launcher mcp v1 <<<"));
    let block_start = content.find("# >>> dsh-launcher mcp v1").unwrap();
    let block = &content[block_start..];
    assert!(block.contains("- insert:"));
    assert!(block.contains("    - id: mcp-playwright"));
    assert!(block.contains("    - id: mcp-remote"));
    assert!(block.contains("        serverName: 'playwright'"));
    assert!(block.contains("        command: 'playwright-mcp'"));
    assert!(block.contains("        url: 'https://example.test/mcp'"));
    assert!(block.contains("        toolCallTimeoutMs: 15000"));
    assert!(block.contains("        reconnect:"));
    assert!(block.contains("          maxAttempts: 3"));
    assert!(block.contains("- id: mcp-playwright\n  disabled: false"));
    assert!(block.contains("- id: mcp-remote\n  disabled: false"));
    // 危险字段不暴露（D10）
    assert!(!block.contains("failOnStartupError"));

    // 重新发现：新行为 managed，且能通过 dump 看见
    let listed = fixture.list();
    let playwright = listed
        .servers
        .iter()
        .find(|server| server.server_name == "playwright")
        .unwrap();
    assert_eq!(playwright.origin, McpOrigin::Managed);
    assert_eq!(playwright.state, McpState::Enabled);
    // managed 行的来源层 = 机器级 patch 文件
    assert_eq!(playwright.layer, fixture.patch_path().display().to_string());
}

#[test]
fn add_start_disabled_declares_disabled() {
    let fixture = Fixture::new("add-disabled");
    fixture.write_user_patch("# 空文件（真实机器级 patch 没有 `[]` 占位符）\n");
    let mut spec = stdio_spec("offline", "offline-mcp");
    spec.start_disabled = true;
    mcp::add(&spec, &fixture.logger()).unwrap();
    let content = std::fs::read_to_string(fixture.patch_path()).unwrap();
    assert!(content.contains("- id: mcp-offline\n  disabled: true"), "{content}");
    let listed = fixture.list();
    assert_eq!(listed.servers[0].state, McpState::Disabled);
    assert_eq!(listed.servers[0].disabled, Some(true));
}

// ==================== 场景 3：幂等 ====================

#[test]
fn repeated_disable_is_unchanged_and_config_bytes_untouched() {
    let fixture = Fixture::new("idempotent");
    fixture.write_user_patch(USER_PATCH);

    // 第一次 disable 外部行 → 只追加定向条目
    let result = mcp::set_state("shadcn", McpAction::Disable, &fixture.logger()).unwrap();
    assert_eq!(result.status, "changed");
    let after_first = fixture.patch_bytes();
    let first_text = std::fs::read_to_string(fixture.patch_path()).unwrap();
    // 用户手写段逐字节不变（config 子树含 !!js 与内嵌注释）
    assert!(first_text.starts_with(USER_PATCH), "{first_text}");

    // 第二次 disable → unchanged，字节与 mtime 都不变
    let mtime_before = std::fs::metadata(fixture.patch_path())
        .unwrap()
        .modified()
        .unwrap();
    let result = mcp::set_state("shadcn", McpAction::Disable, &fixture.logger()).unwrap();
    assert_eq!(result.status, "unchanged");
    assert_eq!(fixture.patch_bytes(), after_first, "幂等操作不得落盘");
    let mtime_after = std::fs::metadata(fixture.patch_path())
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(mtime_before, mtime_after, "幂等操作不得改写文件");

    // config 子树字节一致（状态操作绝不重渲染 config）
    let disabled_text = std::fs::read_to_string(fixture.patch_path()).unwrap();
    assert!(disabled_text.contains("          Authorization: !!js >-\n"));
    assert!(disabled_text.contains("        cwd: !!js process.cwd()"));
    // 受管区块里只有定向条目，没有声明（external 行不产生声明，即不做"收养"）
    let managed = mcp::read_managed_block().unwrap();
    assert!(managed.declares.is_empty(), "external 行不得被收养");
    assert_eq!(managed.directives.len(), 1);
    assert_eq!(managed.directives[0].row_id, "mcp-shadcn");
    assert!(managed.directives[0].disabled);
}

// ==================== 场景 4：remove 二分 ====================

#[test]
fn remove_managed_deletes_declaration_and_directive() {
    let fixture = Fixture::new("remove-managed");
    fixture.write_user_patch(USER_PATCH);
    mcp::add(&stdio_spec("playwright", "playwright-mcp"), &fixture.logger()).unwrap();
    mcp::set_state("playwright", McpAction::Disable, &fixture.logger()).unwrap();

    let result = mcp::remove("playwright", &fixture.logger()).unwrap();
    assert_eq!(result.status, "changed");
    let managed = mcp::read_managed_block().unwrap();
    // 声明与定向同时消失，无 `entry not found` 残留
    assert!(managed.declares.is_empty(), "{:?}", managed.declares);
    assert!(managed.directives.is_empty(), "{:?}", managed.directives);
    // 用户手写段仍在
    let text = std::fs::read_to_string(fixture.patch_path()).unwrap();
    assert!(text.starts_with(USER_PATCH));
    // 目标不再出现在合成树
    assert!(fixture
        .list()
        .servers
        .iter()
        .all(|server| server.server_name != "playwright"));
}

#[test]
fn remove_external_only_drops_directive_and_message_says_declaration_remains() {
    let fixture = Fixture::new("remove-external");
    fixture.write_user_patch(USER_PATCH);
    mcp::set_state("github", McpAction::Disable, &fixture.logger()).unwrap();
    let before_remove = fixture.patch_bytes();

    let result = mcp::remove("github", &fixture.logger()).unwrap();
    assert_eq!(result.status, "changed");
    // 文案必须明示"声明仍在"（D7）
    assert!(result.message.contains("声明仍由"), "{}", result.message);
    // 只删定向：受管区块变成空 → 区块被删除
    let managed = mcp::read_managed_block().unwrap();
    assert!(managed.declares.is_empty());
    assert!(managed.directives.is_empty());
    // 用户手写段逐字节不变
    let after = std::fs::read_to_string(fixture.patch_path()).unwrap();
    assert!(after.starts_with(USER_PATCH));
    assert_ne!(after, String::from_utf8_lossy(&before_remove));
    // 合成树中该行恢复默认启用
    let listed = fixture.list();
    let github = listed
        .servers
        .iter()
        .find(|server| server.server_name == "github")
        .unwrap();
    assert_eq!(github.state, McpState::Enabled);
    assert_eq!(github.origin, McpOrigin::External);
}

// ==================== 场景 5：并发写互斥 ====================

#[test]
fn concurrent_add_and_disable_are_serialized_without_interleaving() {
    let fixture = Fixture::new("concurrent");
    fixture.write_user_patch(USER_PATCH);

    // 先用 add 建立一条受管声明（受管区块与用户手写段同文件）
    mcp::add(&stdio_spec("worker", "worker-mcp"), &fixture.logger()).unwrap();

    let logger = fixture.logger();
    let handles: Vec<_> = (0..2)
        .map(|index| {
            let logger = Arc::clone(&logger);
            std::thread::spawn(move || {
                for _ in 0..12 {
                    if index == 0 {
                        let _ = mcp::set_state("worker", McpAction::Disable, &logger);
                    } else {
                        let _ = mcp::set_state("worker", McpAction::Enable, &logger);
                    }
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let text = std::fs::read_to_string(fixture.patch_path()).unwrap();
    // 无交错写：用户手写段完整、marker 成对且唯一
    assert!(text.starts_with(USER_PATCH), "块外字节被破坏:\n{text}");
    assert_eq!(text.matches("# >>> dsh-launcher mcp v1").count(), 1);
    assert_eq!(text.matches("# <<< dsh-launcher mcp v1 <<<").count(), 1);
    // 受管区块仍能解析（无半截写入）
    let managed = mcp::read_managed_block().unwrap();
    assert_eq!(managed.declares.len(), 1);
    assert_eq!(managed.directives.len(), 1);
}

// ==================== 场景 6：marker 破坏 / 非法输入 ====================

#[test]
fn broken_marker_is_conflict_with_zero_write() {
    // 本测试需要两个互不干扰的临时环境；`Fixture` 持有进程级环境锁，
    // 因此必须**分块**让前一个 `Fixture` 先析构（同线程重入会自锁死）。
    let broken = concat!(
        "# 用户注释\n",
        "# >>> dsh-launcher mcp v1 — 由启动器维护，请勿手工编辑 >>>\n",
        "- id: mcp-a\n",
        "  disabled: true\n",
    );
    {
        let fixture = Fixture::new("broken-marker");
        fixture.write_user_patch(broken);

        let result = mcp::list(&fixture.logger());
        assert!(result.is_err(), "marker 异常时 list 必须 fail loud");
        assert_eq!(
            result.unwrap_err().kind,
            PluginErrorKind::ManagedBlockConflict
        );

        let error = mcp::add(&stdio_spec("x", "x-mcp"), &fixture.logger()).unwrap_err();
        assert_eq!(error.kind, PluginErrorKind::ManagedBlockConflict);
        assert_eq!(error.kind.exit_code(), 7);
        // 零写入
        assert_eq!(
            std::fs::read_to_string(fixture.patch_path()).unwrap(),
            broken
        );
    }

    {
        // 顶层非 YAML 数组：目标不存在时优先报 NotFound(3)
        let fixture = Fixture::new("broken-not-array");
        fixture.write_user_patch("root: {}\n");
        let error = mcp::set_state("a", McpAction::Disable, &fixture.logger()).unwrap_err();
        assert_eq!(error.kind, PluginErrorKind::NotFound);
        assert_eq!(error.kind.exit_code(), 3);
    }
}

#[test]
fn invalid_inputs_return_documented_exit_codes_with_zero_write() {
    let fixture = Fixture::new("invalid");
    fixture.write_user_patch(USER_PATCH);
    let before = fixture.patch_bytes();

    // serverName 非法 → IllegalTransition(2)
    let mut bad_name = stdio_spec("bad name", "x");
    bad_name.transport = "stdio".to_string();
    let error = mcp::add(&bad_name, &fixture.logger()).unwrap_err();
    assert_eq!(error.kind, PluginErrorKind::IllegalTransition);
    assert_eq!(error.kind.exit_code(), 2);

    // serverName 超长（33 字符）
    let long = "a".repeat(33);
    assert_eq!(
        mcp::add(&stdio_spec(&long, "x"), &fixture.logger())
            .unwrap_err()
            .kind,
        PluginErrorKind::IllegalTransition
    );

    // serverName 重复（与用户手写行冲突）→ 2，且错误含冲突来源层
    let error = mcp::add(&stdio_spec("github", "x"), &fixture.logger()).unwrap_err();
    assert_eq!(error.kind, PluginErrorKind::IllegalTransition);
    assert!(error.message.contains("mcp-github"), "{}", error.message);
    assert!(
        error.message.contains(&fixture.patch_path().display().to_string()),
        "{}",
        error.message
    );

    // stdio 缺 command → 2
    let mut no_command = McpAddSpec {
        server_name: "nocommand".to_string(),
        transport: "stdio".to_string(),
        ..Default::default()
    };
    no_command.command = None;
    assert_eq!(
        mcp::add(&no_command, &fixture.logger()).unwrap_err().kind,
        PluginErrorKind::IllegalTransition
    );

    // streamable-http 缺 url → 2
    let no_url = McpAddSpec {
        server_name: "nourl".to_string(),
        transport: "streamable-http".to_string(),
        ..Default::default()
    };
    assert_eq!(
        mcp::add(&no_url, &fixture.logger()).unwrap_err().kind,
        PluginErrorKind::IllegalTransition
    );

    // 未知 transport → 2
    let mut bad_transport = stdio_spec("sse", "x");
    bad_transport.transport = "sse".to_string();
    assert_eq!(
        mcp::add(&bad_transport, &fixture.logger()).unwrap_err().kind,
        PluginErrorKind::IllegalTransition
    );

    // raw-config 与结构化字段互斥 → 2
    let both = McpAddSpec {
        server_name: "both".to_string(),
        transport: "stdio".to_string(),
        command: Some("x".to_string()),
        raw_config: Some("serverName: both".to_string()),
        ..Default::default()
    };
    let error = mcp::add(&both, &fixture.logger()).unwrap_err();
    assert!(error.message.contains("互斥"), "{}", error.message);

    // remove/enable/disable 不存在的 serverName → NotFound(3)
    assert_eq!(
        mcp::remove("nope", &fixture.logger()).unwrap_err().kind,
        PluginErrorKind::NotFound
    );
    assert_eq!(
        mcp::set_state("nope", McpAction::Enable, &fixture.logger())
            .unwrap_err()
            .kind,
        PluginErrorKind::NotFound
    );
    assert_eq!(
        mcp::set_state("nope", McpAction::Disable, &fixture.logger())
            .unwrap_err()
            .kind
            .exit_code(),
        3
    );

    // 非法输入一律零写入
    assert_eq!(fixture.patch_bytes(), before, "非法输入不得写入任何字节");
}

#[test]
fn add_rejected_with_capability_missing_when_prereq_absent() {
    let fixture = Fixture::new("prereq-missing");
    fixture.write_user_patch(USER_PATCH);
    // 删掉前置包目录
    let pkg = fixture
        .dsh_home
        .join("profiles")
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh-mcp-client");
    std::fs::remove_dir_all(&pkg).unwrap();

    let listed = fixture.list();
    assert!(!listed.prereq.installed);
    assert!(!listed.prereq.package.is_empty());
    // 前置缺失不阻塞 list，但要可被前端显示（列表仍返回）
    assert_eq!(listed.servers.len(), 2);

    let error = mcp::add(&stdio_spec("x", "x-mcp"), &fixture.logger()).unwrap_err();
    assert_eq!(error.kind, PluginErrorKind::CapabilityMissing);
    assert_eq!(error.kind.exit_code(), 6);
}

// ==================== 表达式行 / 危险字段 ====================

#[test]
fn expression_disabled_row_is_readonly_and_dangerous_flag_is_reported() {
    let fixture = Fixture::new("expression");
    fixture.write_user_patch(concat!(
        "# 用户手写\n",
        "- insert:\n",
        "    - id: mcp-cond\n",
        "      name: '@deepseek-ai/dsh-mcp-client'\n",
        "      config:\n",
        "        serverName: cond\n",
        "        transport: stdio\n",
        "        command: cond-mcp\n",
        "        failOnStartupError: true\n",
        "\n",
        "- id: mcp-cond\n",
        "  disabled: !!js process.platform === 'win32'\n",
    ));

    let listed = fixture.list();
    let cond = &listed.servers[0];
    // `!!js` 表达式 → 只读展示，disabled = null
    assert!(cond.marks.contains(&McpMark::Expression), "{:?}", cond.marks);
    assert!(cond.marks.contains(&McpMark::Dangerous), "{:?}", cond.marks);
    assert_eq!(cond.disabled, None);
    // 按启用处理（启动器不猜表达式结果）
    assert_eq!(cond.state, McpState::Enabled);

    // 覆盖被拒绝 → IllegalTransition(2) 且零写入
    let before = fixture.patch_bytes();
    let error = mcp::set_state("cond", McpAction::Disable, &fixture.logger()).unwrap_err();
    assert_eq!(error.kind, PluginErrorKind::IllegalTransition);
    assert!(error.message.contains("表达式"), "{}", error.message);
    assert_eq!(fixture.patch_bytes(), before);
}

#[test]
fn server_name_conflict_is_flagged_in_list() {
    let fixture = Fixture::new("conflict");
    // 同一 serverName 声明两次（不同行 id）
    fixture.write_user_patch(concat!(
        "# 用户手写：重复 serverName\n",
        "- insert:\n",
        "    - id: mcp-dup-a\n",
        "      name: '@deepseek-ai/dsh-mcp-client'\n",
        "      config:\n",
        "        serverName: dup\n",
        "        transport: stdio\n",
        "        command: a\n",
        "    - id: mcp-dup-b\n",
        "      name: '@deepseek-ai/dsh-mcp-client'\n",
        "      config:\n",
        "        serverName: dup\n",
        "        transport: stdio\n",
        "        command: b\n",
    ));
    let listed = fixture.list();
    assert_eq!(listed.servers.len(), 2);
    for server in &listed.servers {
        assert!(
            server.marks.contains(&McpMark::Conflict),
            "{}: {:?}",
            server.server_name,
            server.marks
        );
    }
    // add 同 serverName 也被拒绝
    let error = mcp::add(&stdio_spec("dup", "c"), &fixture.logger()).unwrap_err();
    assert_eq!(error.kind, PluginErrorKind::IllegalTransition);
    assert!(error.message.contains("重复"), "{}", error.message);
}

#[test]
fn raw_config_channel_passes_official_fields_verbatim_including_js() {
    let fixture = Fixture::new("raw-config");
    fixture.write_user_patch("# 空（真实机器级 patch 没有 `[]` 占位符）\n");
    let raw = concat!(
        "serverName: rawsrv\n",
        "transport: streamable-http\n",
        "url: https://raw.test/mcp\n",
        "headers:\n",
        "  # 密钥不落盘：走 !!js 取环境\n",
        "  Authorization: !!js >-\n",
        "    (() => `Bearer ${process.env.TOKEN ?? ''}`)()\n",
        "failOnStartupError: true\n",
        "x-unknown-official-future-field: 42\n",
    );
    let spec = McpAddSpec {
        server_name: "rawsrv".to_string(),
        transport: "stdio".to_string(),
        raw_config: Some(raw.to_string()),
        ..Default::default()
    };
    mcp::add(&spec, &fixture.logger()).unwrap();

    let content = std::fs::read_to_string(fixture.patch_path()).unwrap();
    // 原始通道原样透传：`!!js`、内嵌注释、未知字段全部保留
    assert!(content.contains("        # 密钥不落盘：走 !!js 取环境"));
    assert!(content.contains("        Authorization: !!js >-"));
    assert!(content.contains("          (() => `Bearer ${process.env.TOKEN ?? ''}`)()"));
    assert!(content.contains("        failOnStartupError: true"));
    assert!(content.contains("        x-unknown-official-future-field: 42"));

    // 危险字段经原始通道进入 → list 打危险徽章
    let listed = fixture.list();
    let raw_server = listed
        .servers
        .iter()
        .find(|server| server.server_name == "rawsrv")
        .unwrap();
    assert!(raw_server.marks.contains(&McpMark::Dangerous));
    assert_eq!(raw_server.transport, Some(mcp::state::McpTransportView::StreamableHttp));
}

// ==================== G3（审计 SEC-03）：marker 注入与回滚 ====================

/// G3：MCP 原始通道（`raw_config`）含受管区块 marker 时，`add` 必须**拒绝且零写入**。
///
/// 若允许写入，文件会出现重复 marker → 此后所有区块读写判 `Broken`，需人工修文件。
#[test]
fn g3_raw_config_with_managed_marker_is_rejected_with_zero_write() {
    let fixture = Fixture::new("g3-marker");
    // 让区块已存在，以便验证“被拒时文件逐字节不变”
    fixture.write_user_patch(USER_PATCH);
    let before = std::fs::read_to_string(fixture.patch_path()).unwrap();

    // 构造一个内含 mcp 受管区块起始 marker 的原始体
    let marker = dsh_launcher_lib::core::mcp::block::mark_begin();
    let raw = format!(
        "serverName: evilsrv\ntransport: stdio\ncommand: x\nx-note: '{marker}'\n"
    );
    let spec = McpAddSpec {
        server_name: "evilsrv".to_string(),
        transport: "stdio".to_string(),
        raw_config: Some(raw),
        ..Default::default()
    };

    let error = mcp::add(&spec, &fixture.logger()).unwrap_err();
    assert!(
        error.message.contains("marker"),
        "错误信息应指明 marker 问题: {}",
        error.message
    );
    // 零写入：文件与操作前逐字节一致，且 marker 仍成对
    assert_eq!(
        std::fs::read_to_string(fixture.patch_path()).unwrap(),
        before,
        "被拒时文件必须逐字节不变"
    );
    assert_eq!(before.matches(&marker).count(), 0, "前置条件：用户 patch 无受管区块");
    // 被拒后仍无受管区块（未写入），且未新增任何行
    assert_eq!(
        std::fs::read_to_string(fixture.patch_path()).unwrap().matches(&marker).count(),
        0
    );
    // 区块仍可读（未损坏）
    let listed = fixture.list();
    assert!(!listed.servers.iter().any(|s| s.server_name == "evilsrv"));
}

// ==================== 备份与回滚 ====================

#[test]
fn backup_is_written_before_each_change() {
    let fixture = Fixture::new("backup");
    fixture.write_user_patch(USER_PATCH);
    mcp::add(&stdio_spec("playwright", "playwright-mcp"), &fixture.logger()).unwrap();
    let backups = fixture
        .local_appdata
        .join("dsh-launcher")
        .join("backups")
        .join("mcp")
        .join("playwright");
    assert!(backups.is_dir(), "备份落点必须是 %LOCALAPPDATA%\\dsh-launcher\\backups\\mcp\\<serverName>\\<ts>\\");
    let entries: Vec<PathBuf> = std::fs::read_dir(&backups)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert!(!entries.is_empty());
    // 备份里至少有一个 cordis.patch.yml，且内容是变更前的字节
    let has_backup = entries.iter().any(|dir| {
        let file = dir.join("cordis.patch.yml");
        file.is_file()
            && std::fs::read_to_string(&file)
                .map(|text| text.contains("mcp-shadcn"))
                .unwrap_or(false)
    });
    assert!(has_backup, "备份必须包含变更前的 patch 文件");
}

// ==================== 不重启 dsh ====================

#[test]
fn all_operations_report_restarted_false() {
    let fixture = Fixture::new("no-restart");
    fixture.write_user_patch(USER_PATCH);
    let added = mcp::add(&stdio_spec("svc", "svc-mcp"), &fixture.logger()).unwrap();
    let disabled = mcp::set_state("svc", McpAction::Disable, &fixture.logger()).unwrap();
    let enabled = mcp::set_state("svc", McpAction::Enable, &fixture.logger()).unwrap();
    let removed = mcp::remove("svc", &fixture.logger()).unwrap();
    for result in [added, disabled, enabled, removed] {
        assert!(!result.restarted, "MCP 变更不得重启 dsh（OpResult.restarted 恒为 false）");
    }
    // 用户手写段始终逐字节保留
    assert!(std::fs::read_to_string(fixture.patch_path())
        .unwrap()
        .starts_with(USER_PATCH));
}

#[test]
fn reload_mode_requires_restart_for_startup_profile() {
    let fixture = Fixture::new("reload-startup");
    std::fs::write(
        fixture.dsh_home.join("profiles").join("web").join("package.json"),
        r#"{"name":"dsh-profile-web","dependencies":{},"dsh":{"profile":{"bundles":[],"patchReload":"startup"}}}"#,
    )
    .unwrap();
    let listed = fixture.list();
    assert_eq!(listed.reload, mcp::ReloadMode::RequiresRestart);
}
