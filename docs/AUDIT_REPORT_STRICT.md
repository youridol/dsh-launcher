# dsh-launcher 严格模式全链路审查报告

> 审查范围：当前工作区全部内容（含未提交改动）。
> 基线：`HEAD=83c2655` + 工作区未提交改动（14 文件 +514/−26，含未跟踪 `src-tauri/tests/starting_convergence_e2e.rs`）。
> 纪律：只读诊断，未修改任何生产文件（本报告为唯一新增文件）。所有结论均对应真实路径与行号；无法由代码确认者标「待确认」。
> 规模：Rust 50 文件 / 20,804 行；TS+TSX 24 文件 / 7,325 行；Rust 测试 16 文件 / 3,737 行；capabilities 2；CI 3；脚本 6。

---

## 0. 审查摘要

| 层级 | 审查文件数 | P0 | P1 | P2 | 风险热点一句话 |
|---|---|---|---|---|---|
| L1 Rust 后端（commands + core） | 50 全部（深读 22，其余按接口/契约核验） | 0 | 3（**全部已修复**） | 8（**全部已收口**） | 原风险点均已收口：锁毒化、MCP 区块写入、路径 TOCTOU、静默回退 |
| L2 Tauri 集成层 | 5（`tauri.conf.json`、2×capabilities、`build.rs`、`gen/schemas` 对比 + tauri 2.11.5 源码复核） | 1（**定级待裁定**） | 0 | 1（**已修复**） | 内嵌窗口全放行导航（**G1 已修**）；RCE 后果经源码证伪，定级待裁定 |
| L3 前端逻辑 | 15 非 ui 文件 | 0 | 0 | 3（**全部已修复**） | 未处理 Promise 拒绝、断点双源、semver 双实现 —— 均已收口 |
| L4 UI 层 | 11 shadcn 组件 + `index.css`(713) + `App.tsx` | 0 | 0 | 0 | 组件全部被引用、无重复造轮子；dark 主题与 CSS 变量一致 |
| L5 工程与 CI | `Cargo.toml`/`package.json`/`vite.config.ts`/`tsconfig*`/3 workflow/6 脚本 | 0 | 0 | 6（**5 处理 · 1 误报撤销**） | e2e 入 nightly、feature 收窄、`@types/node` 对齐、签名决策为不签；`shadcn` 项经核实为误报已撤销 |
| **合计** | **~95** | **1（待裁定）** | **3（全部已修复）** | **18（17 已收口 · 1 误报撤销）** | 除 SEC-01 定级外，全部条目已处理完毕（含 ENG-05 决策为不签名） |

**三个最优先处置**：SEC-01（内嵌窗口任意源导航；**G1 已修复**，定级待裁定）、SEC-03（MCP 区块可被写坏且回滚被 `?` 绕过）、RT-01（`process.rs` 39 处 `.lock().unwrap()` 毒化即 panic）。

> **执行进度**：§3 的 G0–G8 **全部已实施并验证**（见 §5）。TC-03 采用零依赖契约快照测试；ENG-05 经产品决策为**不签名**。**仅剩 SEC-01 定级待你裁定**（源码已证伪其 RCE 前提）。

> **定级纪律**：本报告中的 P0 **不因「待确认」而降级**；「待确认」只标注证据链的缺口与核验方式，不改变定级与修复优先级。
>
> **SEC-01 定级状态（G0 复核后）**：源码级实证已**推翻**原 P0 定级所依赖的前提（详见 §1 SEC-01 的 G0 复核块）。按证据，最坏后果「远程页面 → 调用 48 个应用命令 → RCE」**不成立**。**本次仅纠正被源码证伪的事实，未擅自改定级**；SEC-01 的最终等级（P0 / P1）**待用户裁定**。G1 的防御性修复已全部实施完毕，不受定级影响。

---

## 1. 问题清单（按严重度降序）

> 字段：**ID | 严重度 | 层级 | 位置 | 现象 | 证据 | 影响 | 修复建议**
> 相同问题跨文件重复时合并为一条并列出全部命中位置。

### SEC-01 | P0→P1 | L2 Tauri 集成层 | `src-tauri/src/commands/dsh.rs:184`（`on_navigation`）、`:192-213`（`on_new_window`）；`src-tauri/capabilities/dsh-web-gui.json:7-13`

> **⚠️ G0 复核结果（源码级实证，推翻原 P0 前提）**：原 P0 定级所依赖的「**闸门② ACL 侧不拦截**」经 Tauri 2.11.5 源码核验**不成立**。
> `tauri-2.11.5/src/webview/mod.rs:1819-1850` 存在**显式来源守卫**（官方原注释）：
> ```rust
> // Check ACL on plugin commands, when the app defined its ACL manifest,
> // or when the request comes from a non-local (remote) origin.  This
> // ensures remote content can never reach custom commands unless an
> // explicit `remote` capability has been configured for them.
> if (plugin_command.is_some() || has_app_acl_manifest || !is_local)
>   && request.cmd != FETCH_CHANNEL_DATA_COMMAND
>   && invoke.acl.is_none()
> { invoke.resolver.reject(format!("Command {} not allowed by ACL", request.cmd)); }
> ```
> 且官方配套测试 `webview/mod.rs:2423-2477` `remote_origin_blocked_for_custom_commands_without_app_manifest` 明确断言：`url: "https://evil.com"` 调用自定义命令 → **被拒**（即使无 AppManifest）。
> 本项目 `app.security` 无 `permissions`/app manifest（`has_app_acl_manifest=false`），但条件中的 `!is_local` 已独立生效 → **远程源页面无法调用 48 个应用命令**。
> 两条 Windows IPC 路径的 `url` 均取自不可伪造的真实页面来源：自定义协议路径取 `Origin` 头（`tauri/src/ipc/protocol.rs:488-496`）；postMessage 回退路径取 WebView2 `args.Source`（`wry-0.55.1/src/webview2/mod.rs:896-910`）。
> **∴ 「远程页面 → RCE」后果链不成立，原 P0 最高后果被排除。**
>
> **定级说明（与用户「P0 不降级」指令的冲突已显式上报）**：本次仅纠正**已被源码证伪的事实**，不处理定级。按证据，SEC-01 适用后果为「任意外部站点可在受信外观的内嵌窗口内渲染」+ 该窗口持 `opener` 能力，属边界/健壮性缺陷；**待用户裁定**是否维持 P0。**G1 两项修复与 SEC-02 已按原计划全部实施完毕**（见 §3-G1 与文末「已实施」节），与定级无关。

- **现象（仍然成立的部分）**：`create_embedded_web_gui_window` 的 `on_navigation(|_url| true)` 曾对**任何** URL 放行；`on_new_window` 只拦截「新窗口请求」，不拦截同窗口导航。内嵌窗口 label 为 `dsh-web-gui-*`，命中 `dsh-web-gui` capability（其 `remote.urls` 仅列 loopback）→ 同窗口导航可把带 capability 的窗口带到任意外部源。
- **证据代码片段（修复前）**：
  ```rust
  .on_navigation(|_url| true);              // 原 dsh.rs:184 —— 全放行（已修）
  ...
  b = b.on_new_window(move |url, _features| { /* 仅新窗口请求分流 */ }); // 原 dsh.rs:192
  ```
  ```json
  "windows": ["dsh-web-gui-*"],
  "remote": { "urls": ["http://127.0.0.1:*","http://localhost:*","https://127.0.0.1:*","https://localhost:*"] }
  ```
- **影响（按源码实证修正后）**：
  - **闸门① launcher 侧（原确定不拦截，已修复）**：`on_navigation` 曾无条件放行，使「本窗口仅作 dsh Web UI 载体」的设计约定失效；**G1 已收紧为仅回环**。
  - **闸门② ACL 侧（已证伪「不拦截」）**：Tauri 对非本地来源的自定义命令**一律拒绝**（上引守卫与测试）→ 远程页面**不能**调用 `set_github_token`/`install_version`/`mcp_add` 等任何应用命令，**不存在 RCE 链**。
  - **残留（仍应修）**：任意外部站点可在受信外观的内嵌窗口内渲染；该窗口仍持有 `opener:default`（可 `open_url`/`reveal_item_in_dir`，scope 含 `http://*`/`https://*`）——属钓鱼面与 UX 边界缺陷。
  - 原报告「`dsh-web-gui` capability 不约束应用命令」的推论**错误**：capability 确实不约束应用命令，但 `!is_local` 分支已独立拦截远程源，两者不是同一道闸门。
- **待确认（已不阻塞定级，且不影响已完成的修复）**：`dsh` Web UI 是否在所有外链上使用 `target=_blank`（仅影响旧版本被触发的频率）。
- **修复建议（已实施，见文末「已实施」）**：① `on_navigation` 收紧为「仅回环 http(s) 放行」；② `probe_web_ready`/`create_web_gui_window` 增加回环 URL 校验。③ 原建议的「命令入口加窗口守卫 / isolation pattern」经源码复核**非必需**（`!is_local` 已覆盖），故未实施，以避免冗余防御与范围外改动。

### SEC-02 | P1 | L1 Rust 后端 | `src-tauri/src/commands/dsh.rs:60-62`（`probe_web_ready`）、`:310-321`（`create_web_gui_window`）

- **现象**：两个命令把前端传入的字符串直接用于后端网络/建窗，未做 loopback 与端口约束。
- **证据代码片段**：
  ```rust
  #[tauri::command]
  pub fn probe_web_ready(url: String) -> bool {
      crate::core::port::web_ready(&url, 2000)   // 接受任意 http:// host:port
  }
  ```
  ```rust
  pub async fn create_web_gui_window(app: tauri::AppHandle, url: String) -> Result<String, String> {
      app2.run_on_main_thread(move || { let _ = create_embedded_web_gui_window(&app, &url); })
  ```
  `port::parse_http_url`（`core/port.rs:98-118`）仅要求 `http://` 前缀，host 任意。
- **影响**：SSRF 原语（后端对任意 host:port 发 TCP + HTTP GET，可用于内网端口探测/指纹）；`create_web_gui_window` 使任意 URL 可进入内嵌窗口，与 SEC-01 叠加放大。
- **修复建议**：两处统一改用 Rust 侧推导的 URL（`process.web_url()` 或 `AppConfig.port` 拼 loopback），或校验 `url` 的 host ∈ {127.0.0.1, localhost, ::1} 且 port == 配置端口，否则拒绝。

### SEC-03 | P1（**已修复**） | L1 Rust 后端 | `src-tauri/src/core/mcp/block.rs:445-457`（`apply`）→ `src-tauri/src/core/plugin/managed.rs:485-502`（`apply_body`）、`:342-382`（`split_outside`/`locate_with_markers`）；`src-tauri/src/core/mcp/mod.rs:341-344` + `:385-405`（校验与回滚）

- **现象（两段叠加）**：(1) 写入体 `body` 未校验是否含受管 marker 子串——`apply_body` 直接拼 `begin_marker + body + end_marker`，而 marker 判定是 `content.matches(marker).count()` 子串计数；任一被 `yaml_quote` 包裹的字段值或 `rawConfig` 含 marker 文本即产生重复 marker。(2) 写后校验读取 `fingerprint_outside(&after_content)?` 使用 `?`，对「区块已损坏」直接早退，**跳过下方的失败回滚块**。
- **证据代码片段**：
  ```rust
  // managed.rs:497-502 —— body 未做 marker 子串校验
  Some(text) => format!("{begin_marker}{eol}{}{eol}{end_marker}{eol}", text.trim_end_matches(...)),
  ```
  ```rust
  // managed.rs:359-367 —— 子串计数判定
  let begin_count = content.matches(begin_marker).count();
  if begin_count != 1 || end_count != 1 { return Located::Broken("... marker 不成对或重复"); }
  ```
  ```rust
  // mcp/mod.rs:341-344 —— 校验失败以 ? 早退，回滚在 :385 之后
  let after_content = managed::read_file(&path).map_err(PluginError::internal)?.unwrap_or_default();
  let after_fingerprint = fingerprint_outside(&after_content)?;   // ← Err 时不回滚
  ```
  （`fingerprint_outside` → `canonical_outside` → `split_outside` → `Err(reason)`，见 `mcp/mod.rs:253-267`。）
- **影响**：`$DSH_HOME/cordis.patch.yml` 被写成重复 marker 后，后续**所有** MCP（及共享）区块读写均返回 `Broken`，MCP 管理持续失败直至用户手工修文件；备份虽已生成但不会自动还原（回滚被绕过）。前端一次性操作即可触发，属数据完整性/可用性缺陷。
- **修复建议**：① `apply_body`（或各家族 `render`）在写入前校验 `body` 不含本家族 `mark_begin()`/`mark_end()` 子串，命中即 `Err`；② 把 `after_fingerprint` 的 `?` 改为把错误压入 `failures`，使损坏路径也走统一回滚；③ 为上述两条各补一条回归测试。

### RT-01 | P1（**已修复**） | L1 Rust 后端 | `src-tauri/src/core/process.rs`（原 39 处 `.lock().unwrap()`）

- **现象**：`ProcessManager` 的写路径一律 `.lock().unwrap()`，毒化即 panic；同文件的读路径（`:79 status`、`:84 current_port`、`:89 web_url`）却用 `.map().unwrap_or()` 容错，`op_lock` 用 `unwrap_or_else(|e| e.into_inner())`。全仓其它模块（`commands/config.rs:18`、`plugin/managed.rs:198,232,234`、`plugin/mod.rs:161,180`）统一采用 `into_inner()`。
- **证据代码片段**：
  ```rust
  *self.status.lock().unwrap() = DshStatus::Starting;      // process.rs:367 写路径 panic
  pub fn status(&self) -> DshStatus {
      self.status.lock().map(|s| *s).unwrap_or(DshStatus::Error)   // :79 读路径容错
  }
  ```
- **影响**：任一后台线程（`spawn_monitor` / `spawn_startup_probe` / `reconcile_once`）在持锁期间 panic 即使后续**所有**状态写路径 panic，监视线程链死亡、状态不再收敛（Tauri 命令的 panic 会被捕获成错误返回，但后台线程静默失效，仅留 crash.log）。
- **修复建议**：`process.rs` 全部 `.lock().unwrap()` 改为 `unwrap_or_else(|e| e.into_inner())`，与仓库既有约定统一；或抽出 `fn lock_or_recover<T>(m:&Mutex<T>)->MutexGuard<T>` 单一实现。

### RT-02 | P2（**已修复**） | L1 Rust 后端 | `src-tauri/src/commands/version.rs:314`

- **现象**：`npm view` 输出 JSON 解析失败被 `unwrap_or_default()` 吞成空列表，随后日志按「成功但为空（请检查网络/镜像源）」记录（`:53-58`），把**解析失败**误报为**网络/镜像问题**。
- **证据代码片段**：
  ```rust
  let versions: Vec<String> = serde_json::from_str(&text).unwrap_or_default();
  ```
- **影响**：npm 输出格式变化（上游行为漂移）会被误诊为网络问题，浪费排查成本。
- **修复建议**：`from_str` 失败时返回具名 `Err`（附原始输出首行片段），或至少记录为 error 而非 warn。

### RT-03 | P2（**已修复**） | L1 Rust 后端 | `src-tauri/src/core/config.rs:104-121`（`AppConfig::load`）、`:110-113`

- **现象**：配置文件 JSON 解析失败 `unwrap_or_default()` → 静默回退默认配置；DPAPI 解密失败 `unwrap_or_default()` → token 静默置空。两条路径**均无日志**。
- **证据代码片段**：
  ```rust
  Err(_) => Self::default(),                                  // :117 解析失败静默
  let dec = decrypt_token(s).unwrap_or_default();             // :111 解密失败静默
  ```
- **影响**：用户配置（端口/开关/镜像）在一次损坏后静默丢失且无任何可观测线索；`load()` 无 `Logger` 依赖，故无法在读取点记录（设计使然）——建议在 `Logger` 可用处对「配置为默认值」做一次提示。
- **修复建议**：`load()` 返回 `(AppConfig, Option<LoadWarning>)` 或增加 `load_checked()`；调用方（`commands/config.rs::get_config` 等）把警告写入日志。

### RT-04 | P2（**已修复**） | L3 前端 | `src/components/AppShell.tsx:183-190`

- **现象**：`void getCurrentWindow().toggleMaximize();` 包在 `try/catch` 中，但 `try/catch` 只能捕获同步抛出；`toggleMaximize()` 返回的 Promise 若 reject（非 Tauri 环境）→ 未处理的 Promise 拒绝。其余同类调用（`WindowControls.tsx` 的 `act()` 包装、`StatusCard.openWithGuide` 的外层 catch）均已正确处理。
- **证据代码片段**：
  ```tsx
  try { void getCurrentWindow().toggleMaximize(); } catch { /* 非 Tauri 环境忽略 */ }
  ```
- **影响**：开发/浏览器预览下控制台未处理拒绝噪音；生产 Tauri 环境不可达。
- **修复建议**：`getCurrentWindow().toggleMaximize().catch(() => {})`。

### RT-05 | P2（**已修复**） | L1 Rust 后端 | `src-tauri/src/core/skill/manage.rs:159-162` 之外，`src-tauri/src/core/skill/sharing.rs:202`（`expect("已确认可读")`）、`src-tauri/src/core/mcp/mod.rs:634`（`expect("validate_transition 已保证存在")`）

- **现象**：生产路径两处 `expect`。`sharing.rs:198-202` 先 `symlink_metadata(...).is_ok() && read_link(...).is_ok()` 再 `read_link(...).expect(...)`——两次调用间存在 TOCTOU，链接被移除即 panic。`mcp/mod.rs:634` 紧邻 `validate_transition(...)?`（`:633`），逻辑上由不变量保证。
- **证据代码片段**：
  ```rust
  if std::fs::symlink_metadata(&view).is_ok() && std::fs::read_link(&view).is_ok() {
      let target = std::fs::read_link(&view).expect("已确认可读");   // sharing.rs:202
  ```
- **影响**：`sharing.rs` 路径仅经 CLI 触达（前端入口已退役）；`mcp` 的 panic 在 `spawn_blocking` 线程内，命令层会返回「任务执行失败」。影响有限，但违背 `tray.rs:29` 明确的「不再 expect 直接 panic」纪律。
- **修复建议**：`sharing.rs` 用 `let Ok(target) = read_link(&view) else { ... }`；`mcp/mod.rs:634` 改 `let Some(row) = row else { return Err(PluginError::internal(...)) }`。

### SEC-04 | P2（**已缓解/已收口**） | L1 Rust 后端 | `src-tauri/src/core/logging.rs:440-454`、`src-tauri/src/core/process.rs:1018-1045`、`src-tauri/src/core/logging.rs:419-438`

- **现象**：dsh 访问 URL（含 `token=`）**明文**写入 `<date>.log`、`last-web-url`、`dsh-web-stdout.log`，并经 `log://line` 事件明文推送前端。这是 `ADR-0009 D5` 明确裁定的产品决策（用户需复制完整带 token 地址外开）。
- **证据代码片段**：
  ```rust
  // logging.rs:440-451 决策记录
  // **决策：日志与前端日志流中的 dsh web 访问地址（含 token）保持明文，不打码。**
  ```
- **影响**：同机同用户可读；**任何**后续「日志导出/上报/截图」场景会泄漏 token（token 为进程级、随 dsh 重启失效，故风险有时限）。
- **修复建议**：保持默认决策不变；建议在导出/复制路径提供「打码」选项（不改默认行为），并在文档中标注该文件的敏感级别。

### SEC-05 | P2（**已修复**） | L1 Rust 后端 | `src-tauri/src/commands/logs.rs:88-97`（`read_log`）、`src-tauri/src/core/skill/editor.rs:309-319`（`open_skill_file`）

- **现象**：校验用 canonical 路径、实际使用非 canonical 路径（经典 TOCTOU 模式）。
- **证据代码片段**：
  ```rust
  let canonical_full = full.canonicalize().map_err(|e| e.to_string())?;
  if !canonical_full.starts_with(&canonical_base) { return Err("非法路径".into()); }
  fs::read_to_string(&full).map_err(|e| e.to_string())      // ← 用的是 full，不是 canonical_full
  ```
  ```rust
  if crate::core::skill::scan::resolve_managed(declared_path).is_none() { return Err(...); }
  let path = declared_path.to_path_buf();                   // ← 后续打开 declared_path
  ```
- **影响**：校验与实际打开之间存在符号链接替换窗口。单用户桌面应用，攻击者需本地并发写权限，实际可利用性低；但属明确的不安全模式。
- **修复建议**：校验通过后改用 `canonical_full` / `canonicalize(declared_path)` 的结果进行读取/打开。

### SEC-06 | P2（**已文档化**） | L1 Rust 后端 | `src-tauri/src/core/skill/import.rs:166-180`

- **现象**：技能导入明确接受 `file://`、绝对路径（`/`）、UNC（`\\`）、盘符路径（`X:`）等本地来源。
- **证据代码片段**：
  ```rust
  let looks_like_repo = url.starts_with("http://") || url.starts_with("https://") || url.starts_with("git@")
      || url.starts_with("ssh://") || url.starts_with("git://") || url.starts_with("file://")
      || url.starts_with('/') || url.starts_with("\\\\") || (url.len() > 2 && url.as_bytes()[1] == b':');
  ```
- **影响**：这是「本地仓库导入」的设计能力（文档已述「任意 git 仓库」）。但在 SEC-01 未定论前，它也是「读取本地任意 git 仓库并把内容复制进技能根（可被 dsh 当指令加载）」的原语，需与 SEC-01 一并评估威胁模型。
- **修复建议**：保持能力；在 ADR-0008 中显式记录该 scheme 白名单的安全含义，并确保导入必须由用户在前端显式确认（当前 `apply` 为显式参数，符合）。

### TC-01 | P2（**已修复**） | L1/L3 类型契约 | `src-tauri/src/commands/config.rs:31`（`ConfigView.github_token`） vs `src/lib/tauri.ts:177-196`（`AppConfig`）

- **现象**：Rust 仍序列化恒为空串的 `github_token` 字段，TS 端 `AppConfig` 已不再声明该字段。
- **证据代码片段**：
  ```rust
  /// 恒为空串（兼容旧字段，避免前端读到明文）
  pub github_token: String,          // config.rs:30-31
  ```
- **影响**：无运行时错误（多余字段被前端忽略），但属明确的死字段，且「兼容旧字段」的理由已不成立（前端从未读取）。是 48 个命令中唯一发现的字段级契约残留。
- **修复建议**：删除 `ConfigView::github_token` 及其赋值（`config.rs:54`）。

### TC-02 | P2（**已修复**） | L1/L3 类型契约 | `src/lib/version.ts:27-75`（`versionSortDesc`） vs `src-tauri/src/core/github.rs:250-335`（`cmp_semver_*`）

- **现象**：semver（含预发布规则：数字段 < 字母段、无预发布 > 有预发布）在 TS 与 Rust **各实现一份**，且两侧都有针对 `rc.9 < rc.10` 的历史修复注释（`version.ts:25` / `github.rs:244-246`）。
- **证据代码片段**：
  ```ts
  // version.ts:27  export function versionSortDesc(a: string, b: string): number
  ```
  ```rust
  // github.rs:290 fn cmp_semver_desc(a: &str, b: &str) -> std::cmp::Ordering
  ```
- **影响**：同一版本数据在两端排序规则可能漂移（历史上两端都曾各自修同一 bug，正是漂移征兆）；github 通道返回顺序与前端展示顺序无一致性保证。
- **修复建议**：排序只在一端负责（推荐 Rust `list_versions` 返回即有序，前端不再排序），或为两侧各补一组共享用例（同一输入集合的期望序）防漂移。

### TC-03 | P2（**已实施**） | L1/L3 类型契约 | 新增 `src-tauri/tests/contract_types_test.rs`；`.github/workflows/ci.yml`

- **现象**：前后端类型全靠**人工注释对齐**（如 `// 对应 Rust DshStatus`），无自动契约校验。审计逐字段核对未发现当前不一致，但属**机制性**风险（Rust 改名 → 前端读到 `undefined`；Rust 加字段 → 前端静默丢弃，二者都不报错）。
- **选型（为何不用 ts-rs/specta）**：本仓**无 TS 测试基建**（无 vitest/jest），引入绑定生成器需新依赖 + 生成物落盘策略 + 与手写 `tauri.ts`（含函数封装与 JSDoc）的共存规则；相对“字段集合是否一致”这一核心目标收益不成比例。故采用**零新依赖、零生产代码侵入**的契约快照测试：直接读双侧源码文本对账。
- **实现**：解析 Rust `pub struct` 的 `pub <field>: …` 并按 `rename_all`（camelCase/lowercase/kebab-case）转为 JSON 键名；解析 TS `export interface` 成员名；对 **33 组显式配对**断言集合相等。
- **覆盖**：每个 DTO 的**字段集合** + `rename_all` 取值语义。
- **不覆盖（已在文件头注明）**：字段**类型**（`Option<T>` ↔ `T | null`）、可选性（`?`）、嵌套泛型；这些需绑定生成或 JSON Schema 快照。
- **非空洞性证明**（三道防线）：
  1. `契约解析器非空校验`：断言解析器真能提出 `SkillEntry` 的 `whenToUse`/`overriddenBy`（防“解析返回空集→假绿”）；
  2. `契约测试能检出人为漂移`：构造缺失字段，断言精确报出；
  3. **端到端实证**：将 `tauri.ts` 的 `editorPromptSeen` 改名为 `editorPromptSeenRENAMED`（模拟真实漏改），测试立即 FAILED 并报 `前端缺字段: ["editorPromptSeen"]` / `后端缺字段: ["editorPromptSeenRENAMED"]`；随后已还原。
- **验证**：`contract_types_test` **4 passed**；`cargo check --all-targets` 0 警告；已加入 `ci.yml` 测试清单。
- **写测试过程中的两次自纠**（恰好证明测试在工作）：`panic!` 格式串未转义 `}`；`lowercase` 规则误传 snake_case 输入（应用于枚举变体 `StreamableHttp`）。

### AR-01 | P2 | L1 架构一致性 | `src-tauri/src/core/mcp/*`（11 处 `use crate::core::plugin::*`：`block.rs:17`、`entry.rs:17`、`mod.rs:36-38`、`state.rs:14-15`、`validate.rs:17,126`）、`core/skill/manage.rs:14`、`core/skill/sharing.rs:25-26`

- **现象**：`mcp` 子树对 `plugin` 子树的 `managed`（区块通用层）、`dump`、`state::PluginError` 形成编译期耦合；`skill` 亦依赖 `plugin::managed` / `plugin::state`。`docs/DESIGN.md §3.1` 未禁止此依赖（记为「受管区块通用层」），但与「禁止跨模块耦合」的既定约定存在张力。
- **证据代码片段**：
  ```rust
  use crate::core::plugin::managed::{self, BlockFamily, BlockOutcome};   // mcp/block.rs:17
  use crate::core::plugin::state::{PluginError, PluginErrorKind};        // mcp/mod.rs:38
  ```
- **影响**：`mcp`/`skill` 的任何变更都可能波及 `plugin`；通用层（`managed`/`dump`/错误类型）语义上不属于 `plugin`。
- **修复建议**：把 `managed`、`dump`、`PluginError`/`PluginErrorKind` 上提为独立模块（如 `core/blockstore`、`core/apperror`），`plugin`/`mcp`/`skill` 三者对等依赖；或至少在 `DESIGN.md §3.1` 把「通用层归属 plugin」显式写成允许的例外。

### ENG-01 | P2（**已修复**） | L5 工程与 CI | `src-tauri/tests/starting_convergence_e2e.rs`（116/227 行）；`.github/workflows/nightly.yml`（`$tests` 清单）

> **勘误（本轮复核）**：初稿称该文件「含两个 `#[test]`（**非** `#[ignore]`）」——**错**。
> 实际为 `#[test]` + `#[ignore = "需要真实 deepseek-harness 安装目录 + node（本地/nightly 集成验证）"]`
> （行 116-117 与 227-228）。初稿的 grep 模式是 `#\[ignore\]`，**未匹配**带 `= "…"` 的
> `#[ignore = …]` 形式，因此漏看并误判。真正的缺口不变：它既不在 `ci.yml` 也不在
> `nightly.yml` 的 `--ignored` 清单 → 永不执行。

- **现象（修正后）**：两个用例已正确标 `#[ignore]`（需真实 dsh 环境），但 `nightly.yml` 的
  `$tests` 列表只有 `github_tag_test` / `npm_versions_test` / `direct_node_capture_test`，
  **未包含本文件** → 夜间流水线也不会跑它。
- **证据代码片段**：
  ```rust
  #[test]
  #[ignore = "需要真实 deepseek-harness 安装目录 + node（本地/nightly 集成验证）"]
  fn 冷启动超过八秒上限时状态仍收敛为运行中() {      // :116-118
  ```
- **影响**：v0.9.1 的核心修复（Starting 收敛、旧 token 不复用）缺少任何流水线守护；
  文件内 `isolate_data_dir` 已用 `DSH_LAUNCHER_DATA_DIR` 做隔离缝，并入夜间验证安全。
- **修复（已实施，见 §5）**：加入 `nightly.yml` 的 `$tests` 列表（它本就不是 hermetic 测试，
  不应进 `ci.yml` 门禁；放入带 `--ignored` 的夜间环境集成流程是正确的归属）。

### ENG-02 | P2（**已修复**） | L5 工程与 CI | `package.json`（`@types/node`） vs `.github/workflows/*.yml`（`node-version: 22`）

- **现象**：类型定义指向 Node 26，构建与 CI 运行在 Node 22。
- **影响**：`vite.config.ts` 等构建期代码可能引用 Node 22 不存在的 API 而通过类型检查；本仓库 `vite.config.ts` 仅用 `process.env`/`path`，当前无实际风险。
- **修复建议**：`@types/node` 对齐到 CI 的 Node 主版本（`^22`），或统一升级 CI Node。

### ENG-03 | P2（**撤销：报告有误**） | L5 依赖必要性 | `package.json`（`shadcn` 于 devDependencies）

> **勘误（本轮复核）**：初稿称「`shadcn` 是 CLI 工具，源码零引用」——**错**。
> `src/index.css:3` 有 `@import "shadcn/tailwind.css";`（shadcn 作为 **CSS 运行时依赖**被引用）。
> 初稿的 grep 只查了 `src/` 内的 `"shadcn"` 字面量，漏掉了这条 CSS 导入。
> **实施时已被构建拦住**（移除后 `vite build` 报 `Can't resolve 'shadcn/tailwind.css'`），
> 已回滚恢复（并在验证中确认 `npm run build` 重新通过）。

- **结论**：`shadcn` 是**必需**依赖（提供 `shadcn/tailwind.css` 主题基座），**不得移除**。
  本项从问题清单中撤销；保留此条目以记录初稿错误与纠正依据（零编造纪律）。

### ENG-04 | P2（**已修复**） | L5 工程与 CI | `src-tauri/Cargo.toml:31`（`windows` features）、`:41-42`（dev-dependencies）

- **现象**：`Win32_System_Environment` 在 `dependencies` 与 `dev-dependencies` 均声明，但 `src/` 与 `tests/` **零引用**；`Win32_Graphics_Gdi` 在 `dependencies` 中 `src/` 零引用（仅 `tests/icon_window_test.rs` 需要，dev-dependencies 已另行声明）。
- **证据代码片段**：
  ```toml
  windows = { version = "0.61", features = [..., "Win32_System_Environment", ...] }
  ```
- **影响**：编译期无用 feature 展开（体积/编译时），且与注释「GDI 用于 CreateIcon」不符（`CreateIcon` 属 `Win32_UI_WindowsAndMessaging`）。
- **修复建议**：从 `dependencies` 移除 `Win32_System_Environment`、`Win32_Graphics_Gdi`（保留 dev-dependencies 中测试所需项），并同步更新注释。

### ENG-05 | P2（**已收口：决策为不签名 + 改以校验和替代**） | L5 发布配置 | `.github/workflows/release.yml`、`src-tauri/tauri.conf.json`、`README.md`

- **现象**：NSIS 安装包与便携 exe 均**未做代码签名**；未引入 `tauri-plugin-updater`（全仓零引用），故不存在 updater 签名配置。
- **影响**：Windows SmartScreen 会对未签名 exe 告警；用户无法验证二进制来源。updater 维度在本项目**不适用**（无自动更新链路）。
- **决策（产品所有者，本轮）**：**不购买证书、不做代码签名** —— 接受 SmartScreen 告警，不引入任何签名密钥/凭据依赖。
- **已实施的替代措施（免费、无外部资产）**：`release.yml` 新增「生成并上传校验和（`SHA256SUMS.txt`）」步骤（安装包 + 便携包的 SHA-256，标准 `sha256sum -c` 格式），并在 README「安装」节向用户说明 SmartScreen 提示的成因与核对方式 —— 使“来源可验证”不以付费证书为前提。该步骤的 PowerShell 逻辑已本地实测（含 NSIS 产物 glob 匹配、`sha256sum -c` 回校通过）。
- **因此本项不作为待办**；“修复建议”保留如下备查，若未来分发策略变更再启用（属新决策，非本审计遗留）。
- **备查（未来若启用）**：
  - 标准证书：`bundle.windows.certificateThumbprint` + `digestAlgorithm`（官方注：签名必需，建议 SHA-256）+ `timestampUrl`/`tsp`；证书需导入 CI 的 `Cert:\CurrentUser\My`（私钥来自付费 CA 证书，属外部资产）。
  - 云签名：`bundle.windows.signCommand`（`config.rs:1008-1022` 的 `CustomSignCommandConfig`；`%1` 替换为待签名二进制路径），适合 EV/不希望私钥落 CI。
  - 若引入 `tauri-plugin-updater`：**另需** `plugins.updater.pubkey` + 私钥签名（minisign 密钥对，与代码签名证书**不是**同一物，私钥不得入库）。
- **文档一致性检查**：已 grep 全仓 `README/CHANGELOG/CONTEXT/docs`，**未发现**任何“已签名/已验证签名”的不实声明，故无需修正文档。

---

## 2. 死代码与重复代码索引

### 2.1 死代码

| 类型 | 位置 | 说明 | 处理建议 | 状态 |
|---|---|---|---|---|
| 未使用函数 | `src-tauri/src/commands/skill.rs:16-19` | `#[allow(dead_code)] fn format_error`，注释自述「仅为兼容旧共享命令保留」 | 删除 | **✅ 已删**（G2；并收归为 `PluginError::ipc_message()`） |
| 未使用类型别名 | `src-tauri/src/commands/skill.rs:77` | `pub type SkillEntryView = SkillEntry;` 全仓无引用 | 删除 | **✅ 已删**（G2） |
| 无前端入口的 IPC | `src-tauri/src/lib.rs` 注册 `skill_forget_source` + `src/lib/tauri.ts` | 后端命令与 TS 封装均存在，但**组件层 0 调用** → 来源记录无法从 UI 移除 | 补 UI 入口 / 标注 CLI-only | **✅ 已补 UI**（G2：来源列表增「移除记录」按钮 + 二次确认） |
| 前端未使用的封装 | `src/lib/tauri.ts`（`skillImportUrl`） | 组件统一走 `skillImportBatch`，单条导入封装 0 引用 | 保留（对称 API，ADR-0008 单条导入语义在） | **⚪ 保留（有意）** |
| 未使用导出常量 | `src/lib/panel-layout.ts`（`MOBILE_MAX_WIDTH` / `SPLIT_PANEL_MIN_WIDTH`） | 两者均非全仓零引用：`MOBILE_MAX_WIDTH` 未被外部用（`useIsMobile` 硬编码 640）；`SPLIT_PANEL_MIN_WIDTH` **仅模块内**用（初稿误称零引用） | 消除断点双源 / 去 export | **✅ 已处理**（G7：`useIsMobile` 改引常量；`SPLIT_PANEL_MIN_WIDTH` 去 `export` 保留内部使用） |
| 仅内部使用的导出 | `src/lib/version.ts`（`parseVersionSegments`） | 仅被同文件 `versionSortDesc` 使用 | 去 `export` | **✅ 已随模块删除**（G7/TC-02：整个 `lib/version.ts` 已删） |
| 契约残留字段 | `src-tauri/src/commands/config.rs:30-31` | `ConfigView.github_token` 恒空串，TS `AppConfig` 已无此字段 | 删除字段与赋值 | **✅ 已删**（G2/TC-01） |
| 未使用 crate feature | `src-tauri/Cargo.toml` | `Win32_System_Environment`（0 引用）、`Win32_Graphics_Gdi`（src 0 引用） | 见 ENG-04 | **✅ 已删**（G8/ENG-04；dev-deps 保留 GDI 供 tests） |
| 退役但保留的模块 | `src-tauri/src/core/skill/sharing.rs`（1,079 行）+ `core/skill/mod.rs:30-34` 再导出 | 前端入口已退役（CONTEXT），仅 `cli.rs` 使用 status/apply/migrate/repair-links | 在 ADR 中写明废弃时限；或标 `#[deprecated]` | **⚪ 维持（有意保留）**：属产品决策，不属本次整改范围 |

### 2.2 重复代码

| 类型 | 位置 | 说明 | 处理建议（建议落位） | 预估收益 |
|---|---|---|---|---|
| 错误格式化 ×3 | `commands/plugin.rs:13-15`、`commands/mcp.rs:16-18`、`commands/skill.rs:16-19` | 三份**逐字相同**的 `format!("[{}] {}", error.kind.exit_code(), error.message)` | 抽取到 `core/plugin/state.rs` 旁的 `impl Display for PluginError`，或 `commands/mod.rs` 公共 helper | 消除 3 份同实现与 1 份死代码 |
| 原子写 ×2 | `core/plugin/managed.rs:420-433`（`write_atomic`，临时名硬编码 `.yml.tmp`） vs `core/skill/manage.rs:69-85`（`write_atomic_md`） | `manage.rs:64-68` 注释自述「同模式，但 `managed::write_atomic` 临时后缀对 `SKILL.md` 语义错位」，故复制一份 | 泛化 `managed::write_atomic(path, content, tmp_suffix)` 或上提到 `core/fsutil` | 消除同模式双实现 |
| semver 比较 ×2 | `src/lib/version.ts:27-75` vs `src-tauri/src/core/github.rs:250-335` | 两端各实现一套含预发布规则的降序比较 | 见 TC-02（单端排序 或 共享用例） | 防止排序规则漂移 |
| `OpResult` 形状 ×2 | `core/plugin/mod.rs:45-52`（`package`） vs `core/mcp/mod.rs:64-71`（`server_name`） | 同名字段语义不同、序列化形态近似 | 保留（语义确不同）；若合并需泛型 `OpResult<T>`，收益有限——建议仅在注释交叉引用 | 低（建议不改） |
| 事件载荷类型 ×2 | `src-tauri/src/core/events.rs:59-98`（`InstallPhase`/`ProgressPayload`） vs `src/lib/install.ts:7-15`（`InstallPhase`/`InstallProgress`） | 前后端各手写一份（含 `PHASE_LABELS` 中文字面量） | 见 TC-03（绑定生成） | 契约漂移防护 |
| PowerShell 进程查询模式 ×3 | `core/process.rs:176-189`（`Get-NetTCPConnection`）、`:914-934`（`Get-CimInstance Win32_Process`）、`:1175-1188`（`tasklist`） | 三处「构造 hidden powershell + run + decode + parse」同构 | 抽 `core/process.rs` 内 `fn ps_query(script:&str) -> Option<String>` | 可读性与一致性 |

---

## 3. 整改计划（plan-doc）

> 分组按**功能模块**（与 `docs/DESIGN.md §3` 的模块划分对齐）。每组含：目标 / 涉及文件 / 改动要点 / 验证方式 / 依赖顺序。
> 依赖顺序说明：`G0 → G1 → …`，`→` 表示建议的合并/先后（含测试与文档同步）。不包含审查范围外的顺手重构。

### G0 · Tauri 2.11.5 源码级复核（**已执行，已完成**，结论见 §1 SEC-01 与 §5）

- **目标**：核验远程源能否调用应用自定义命令（原 SEC-01 P0 的关键前提）。
- **结果**：已核验 —— **不能**（`!is_local` 守卫，见 §5）。原「闸门② 不拦截」推论被推翻。
- **对定级**：未改；待用户裁定。
- **涉及文件**：无（仅阅读 Tauri 2.11.5 源码 / 最小复现验证）
- **改动要点**：补证 `authority.rs::resolve_access` 对应用命令的处理路径；确认外链跳转形态。
- **验证方式**：`tauri-2.11.5/src/ipc/authority.rs::resolve_access` 阅读，或 `dsh-web-gui-*` 窗口加载非 loopback 页面执行 `invoke('get_config')`。
- **依赖顺序**：无（最先）；**并行于 G1 执行**。

### G1 · 内嵌 Web GUI 窗口与 IPC 边界（安全最高优先）

- **目标**：内嵌窗口只能停留在受信 loopback 源；IPC 不接受前端任意 URL/路径。
- **涉及文件**：`src-tauri/src/commands/dsh.rs`（`create_embedded_web_gui_window:154-282`、`probe_web_ready:60`、`create_web_gui_window:310`）、`src-tauri/capabilities/dsh-web-gui.json`
- **改动要点**：
  1. `on_navigation` 收紧为 loopback（`127.0.0.1`/`localhost`/`::1`）且端口 == 配置端口；非 loopback → `open_url` 外开 + 返回 `false`（与 `on_new_window` 现有分流语义一致）。
  2. `probe_web_ready` / `create_web_gui_window` 增加 URL 校验（host ∈ loopback 且 port == 配置端口），或改为不接受 url 参数、由 Rust 内部推导。
  3. 对 `dsh-web-gui-*` 窗口**不暴露任何应用命令**（必需项，非条件项）：在命令入口加「窗口 label / 来源」守卫（拒绝 `dsh-web-gui-*`），或在 `create_embedded_web_gui_window` 启用 isolation pattern —— 使其**不依赖** ACL 语义。
- **验证方式**：`cargo test --lib` + 手工：在内嵌窗口点击一个非 loopback 链接，确认在外开浏览器且窗口不离开 loopback；新增单测覆盖 URL 校验函数。
- **依赖顺序**：**不依赖 G0**；与 G2 合并为一个 PR（同为 IPC 边界）。**（已实施，见 §5）**

### G2 · commands 层错误契约与契约残留

- **目标**：错误格式化单一实现；删除残留字段。
- **涉及文件**：`src-tauri/src/commands/{plugin,mcp,skill}.rs`、`src-tauri/src/commands/config.rs`、`src/lib/tauri.ts`
- **改动要点**：① 抽出 `PluginError` 的 `Display`（或公共 helper），删除 3 份 `format_error` 与其 `#[allow(dead_code)]`；② 删除 `ConfigView.github_token` 与 `SkillEntryView`；③ 决定 `skill_forget_source` 的去向（补 UI 或标注 CLI-only）。
- **验证方式**：`cargo check --all-targets`（零警告）+ `cargo test --lib`；前端 `tsc --noEmit`。
- **依赖顺序**：可与 G1 并行；建议在 G1 之后合入以减少冲突。

### G3 · MCP/插件受管区块写入安全（SEC-03）

- **目标**：写入体不得破坏 marker 结构；校验失败必须回滚。
- **涉及文件**：`src-tauri/src/core/plugin/managed.rs`（`apply_body:485`、`write_atomic:420`）、`src-tauri/src/core/mcp/block.rs`（`render:379`、`apply:445`）、`src-tauri/src/core/mcp/mod.rs`（`apply_with_verification:310-405`）、`src-tauri/src/core/skill/manage.rs`（`write_atomic_md:69`）
- **改动要点**：① `apply_body` 写入前校验 `body` 不含 `begin_marker`/`end_marker` 子串，命中即 `Err`（并在各家族 `render` 出口同样自检）；② `mcp/mod.rs:344` 的 `?` 改为把错误压入 `failures` 以进入统一回滚；③ 泛化原子写（顺带解决 2.2 的 `write_atomic` 重复）。
- **验证方式**：新增回归测试——`mcp_add` 的 `rawConfig`/`command` 含 marker 文本时：写入被拒（或写入后自动回滚），且文件 marker 仍成对；`cargo test --test mcp_pipeline_test` + `--test plugin_pipeline_test` 全绿。
- **依赖顺序**：独立；建议与 G2 同批（同属 core 写盘链路）。

### G4 · 进程管理器健壮性（RT-01）

- **目标**：锁毒化不再导致 panic 与状态机静默死亡。
- **涉及文件**：`src-tauri/src/core/process.rs`
- **改动要点**：39 处 `.lock().unwrap()` 统一为 `unwrap_or_else(|e| e.into_inner())`（或抽 `lock_or_recover`）；保持读路径现有容错语义。
- **验证方式**：`cargo test --test concurrency_test` + `--test starting_convergence_e2e`（若已按 ENG-01 纳入）+ `cargo test --lib`；静态检查 `grep -c "\.lock()\.unwrap()" core/process.rs` 归零。
- **依赖顺序**：独立；建议与 ENG-01 同批（e2e 是回归守护）。

### G5 · 配置与日志可观测性（RT-02/RT-03/SEC-04）

- **目标**：解析/解密失败不再静默；token 明文的边界显式化。
- **涉及文件**：`src-tauri/src/core/config.rs`、`src-tauri/src/commands/version.rs`、`src-tauri/src/core/logging.rs`
- **改动要点**：① `AppConfig::load` 增加 `load_checked()` 或返回警告，由调用方记日志；② `version.rs:314` 解析失败返回具名 `Err`；③ 在日志导出/复制路径（若存在）提供打码选项，默认不变（遵守 ADR-0009 D5）。
- **验证方式**：`cargo test --lib`；手工：破坏 `config.json` 后启动，确认日志出现「配置解析失败，已回退默认」。
- **依赖顺序**：独立。

### G6 · 技能路径校验与导入边界（SEC-05/SEC-06/RT-05）

- **目标**：消除 TOCTOU 与生产路径 `expect`。
- **涉及文件**：`src-tauri/src/commands/logs.rs`、`src-tauri/src/core/skill/editor.rs`、`src-tauri/src/core/skill/sharing.rs`、`src-tauri/src/core/mcp/mod.rs`
- **改动要点**：① `read_log` 读 canonical 路径；② `open_skill_file` 用 canonicalize 结果打开；③ `sharing.rs:202` / `mcp/mod.rs:634` 的 `expect` 改为显式 `Err`/`else`；④ ADR-0008 补记 scheme 白名单的安全含义。
- **验证方式**：`cargo test --lib` + `--test path_inject_test` + `--test skill_write_pipeline_test`。
- **依赖顺序**：独立。

### G7 · 前端运行时与类型契约（RT-04/TC-01/TC-02/TC-03）

- **目标**：未处理拒绝清零；类型契约机制化。
- **涉及文件**：`src/components/AppShell.tsx`、`src/lib/version.ts`、`src/lib/tauri.ts`、`src/lib/panel-layout.ts`、`src/hooks/useIsMobile.ts`
- **改动要点**：① `AppShell.tsx:186` 加 `.catch`；② `useIsMobile` 引用 `MOBILE_MAX_WIDTH`（消除断点双源）；③ 排序收敛到单端（或补共享用例）；④（可选，成本较高）引入绑定生成或契约快照测试。
- **验证方式**：`npx tsc --noEmit` + `npm run build`。
- **依赖顺序**：独立；TC-03 若采纳绑定生成，需单独立项。

### G8 · 工程与 CI 卫生（ENG-01…ENG-05）

> 注：ENG-05 经产品决策为**不签名**（见 §1）；ENG-01/02/04 已实施，ENG-03 经核实为误报已撤销。

- **目标**：测试无悬空、依赖无冗余、发布可验证。
- **涉及文件**：`.github/workflows/ci.yml`、`.github/workflows/nightly.yml`、`src-tauri/Cargo.toml`、`package.json`、`.github/workflows/release.yml`
- **改动要点**：① `starting_convergence_e2e` 纳入 CI 或 nightly（二选一）；② `@types/node` 对齐 Node 22；③ 移除未使用 crate features；④ `shadcn` 移出 devDependencies 或注明理由；⑤（需决策）接入代码签名。
- **验证方式**：CI 全绿；`cargo check --all-targets` 零警告。
- **依赖顺序**：G8① 建议与 G4 同批；其余独立。

### 建议批次顺序

```
G0（确认 ACL）
  └─→ G1（窗口/IPC 边界）──┐
G2（commands 契约）───────┼─→ G3（受管区块写入）
G4（process 锁）── G8①（e2e 入 CI）  ─┘
G5（配置/日志可观测性）· G6（路径校验）· G7（前端/契约）· G8②-⑤（依赖/签名）
```

---

## 4. 未覆盖范围与不确定性

### 4.1 未审查到的部分及原因

| 范围 | 原因 |
|---|---|
| `src-tauri/src/core/plugin/{mod,managed,registry,spec,sync}.rs`、`core/mcp/{mod,block,state}.rs`、`core/skill/{import,frontmatter,scan,manage,update,source}.rs` 的**全部业务分支** | 逐行不现实（约 9,000 行）。已深读：文件头契约、公共 DTO、写入/校验/回滚主链、安全边界（`yaml_quote`、`validate_row_id`、`resolve_managed`、marker 处理）。**未覆盖**：各状态机的全部分支组合、`sync.rs` 的 upstream 同步规则细节、`frontmatter` 的逐行外科手术边界用例、`update.rs` 的三类清单比较。 |
| `core/toolchain.rs`（818 行）下载/解压/安装细节 | 仅核验了安装入口（`commands/toolchain.rs`）与 PATH 注入（`core/pathutil.rs`）。**未覆盖**：zip 解压路径校验、镜像 URL 拼接、`uninstall_*` 的注册表枚举细节。 |
| `core/stream.rs`/`core/text.rs`/`core/port.rs` 的非主路径 | 已读全文（均 <300 行），但未逐一构造边界输入验证。 |
| `src-tauri/src/cli.rs`（758 行） | 仅核验其 `core` 依赖与命令路由；未逐参数审阅。 |
| `src/components/{McpPanel,SkillsPanel,StatusCard,PluginsPanel,VersionPanel,ToolchainPanel,SettingsPanel,LogPanel}.tsx` 全部渲染分支 | 已核验 IPC 调用、事件订阅、错误处理、未使用导出；**未覆盖**：表单校验的完整性、各面板的竞态细节（除 StatusCard 的 `openSeq` 机制外）。 |
| `src-tauri/tests/*`（3,737 行） | 未逐条评估测试**质量**（仅核验了 CI 覆盖缺口与 `version_sync_test` 的独立性）。 |
| 二进制产物、`node_modules/`、`src-tauri/target/`、`dist*/` | 非源码；`dist-bundles/` 63MB 已确认被 `.gitignore` 忽略且未跟踪。 |
| updater 签名链路 | 项目**未引入** `tauri-plugin-updater`（全仓零引用）→ 该维度不适用，非漏审。 |

### 4.2 「待确认」条目汇总

| 编号 | 待确认内容 | 需要的信息 / 验证方式 |
|---|---|---|
| SEC-01 | ~~Tauri 2.11.5 `authority.rs::resolve_access` 的源码级处理路径~~ **已核验并关闭** | 已读 `tauri-2.11.5/src/webview/mod.rs:1819-1850`（`!is_local` 来源守卫）+ 官方测试 `:2423-2477`；结论：**远程源无法调用应用命令**。 |
| SEC-01 | `dsh` Web UI 自身是否在所有外链上使用 `target=_blank`（仅影响旧版本被真实点击触发的**频率**，不影响漏洞存在性） | 需 dsh 上游 UI 代码或抓包；本仓库不含 dsh 源码。 |
| SEC-01 | **定级裁定**：源码实证推翻 RCE 后果后，SEC-01 保持 P0 还是降为 P1 | **需用户裁定**（本次未擅自改级；G1 修复已完成，与定级无关）。 |
| SEC-03 | ~~`apply_body` 写坏 marker 后是否有其它路径触发回滚~~ **已修复（G3）** | 已在唯一写入路径 `apply_body` 拒绝 marker 子串；写后校验不再 `?` 早退。 |
| RT-01 | ~~毒化 panic 的实际可达路径~~ **已修复（G4）** | 39 处 `.lock().unwrap()` → `lock_or_recover`（容忍中毒），并有中毒注入单测。 |
| AR-01 | `DESIGN.md §3.1`「禁止跨模块耦合」是否允许 `mcp`/`skill` 依赖 `plugin::managed` | 需项目所有者确认约定解释（文档未显式豁免）。 |
| TC-03 | 是否接受引入绑定生成（`ts-rs`/`specta`） | 需产品/架构决策。 |
| SEC-04 | ~~是否在日志导出路径增加打码~~ **已实施（G5）** | 新增「导出打码版」入口；默认明文口径不变（ADR-0009 D5）。 |
| ENG-05 | ~~是否需要代码签名证书~~ **已收口：决策不签** | 产品所有者决定不购买/不签名（接受 SmartScreen 告警）；备查配置见 §1 ENG-05。 |
| TC-03 | ~~是否接受引入绑定生成~~ **已实施（未采用绑定生成）** | 改用零依赖契约快照测试 `contract_types_test`（33 组对账）；已入 `ci.yml`。选型理由见 §1 TC-03。 |
| 勘误1 | `starting_convergence_e2e` 是否为 `#[ignore]`（初稿误判为非 ignore） | 已核验：**是** `#[ignore = "…"]`；已加入 nightly 的 `--ignored` 清单（G8/ENG-01）。 |
| 勘误2 | `shadcn` devDependency 是否未被使用（初稿称零引用） | 已核验：**被使用**（`src/index.css:3` `@import "shadcn/tailwind.css"`）；移除会使 `vite build` 失败，已恢复。ENG-03 撤销。 |

### 4.3 已确认无问题的检查项（供对照，避免重复排查）

- **`Mutex` 持锁跨 `await`**：不存在。`core/` 全为同步代码，`commands/*` 一律 `spawn_blocking` 包裹；无 `tokio` 依赖（`Cargo.toml` 注释记录了移除理由）。
- **`unsafe` 使用**：共 28 处 `unsafe` 块，全部位于 Win32 FFI（`config.rs` DPAPI、`pathutil.rs` 注册表、`dsh.rs` HICON、`editor.rs` ShellExecuteW、`text.rs` GetACP、`toolchain.rs` 注册表），**每处均带 `SAFETY` 注释**（`config.rs` 6、`pathutil.rs` 11、`toolchain.rs` 6、`dsh.rs` 2、其余各 1）。
- **前端 `any` 扩散**：0 处（`: any` / `as any` / `<any>` 均无）；`as` 断言均为受控的枚举字面量或 DOM 事件目标。
- **事件订阅清理**：全部经 `hooks/useTauriEvent.ts` 统一实现，含竞态保护（`disposed` 标志 + resolve 后立即解绑）与错误记录；各面板无手写 `listen`。
- **shadcn/ui 使用**：11 个 `components/ui/*` 组件**全部被引用**（badge 6 / button 11 / card 3 / dialog 5 / input 5 / label 4 / progress 3 / scroll-area 1 / separator 8 / sonner 1 / switch 4 处引用），无未使用组件，未发现重复造轮子。
- **dark 主题一致性**：`App.tsx:66` 强制 `document.documentElement.classList.add("dark")`，与 `index.css:6` 的 `@custom-variant dark (&:is(.dark *))` 及 `:46 .dark { --background … }` 变量块一致；`Toaster theme="dark"`（`App.tsx:119`）同步。
- **JSON DTO 字段级对齐**：逐字段核对了 `PluginState`/`RowState`/`Origin`/`SpecKind`/`PluginView`/`PluginList`/`PluginSource`/`SyncRecord`/`OpResult`/`SyncReport`/`SyncItemResult`、`SkillEntry`/`SkillList`/`RootStatus`/`ToggleReport`/`DeleteReport`/`ImportReport`/`SkillImportPlan`/`FileDiff`、`McpServerView`/`McpListResult`/`McpPrereq`/`McpAddSpec`/`McpOpResult`/`McpState`/`McpOrigin`/`McpMark`/`ReloadMode`/`McpTransportView`、`ConfigView`/`LogFile`/`DshStatus`/`ToolchainItem`/`DshVersion`/`InstallPaths`/`BatchResult`，**未发现字段名或枚举字面量不一致**（除 TC-01 的残留字段）。
- **命令注册对账**：`lib.rs` `generate_handler!` 注册 48 个命令；`src/lib/tauri.ts` 的 48 个 `invoke(...)` 名称与之一一对应，**无未注册的 `#[tauri::command]`、无指向不存在命令的调用**。
- **CSP**：`tauri.conf.json` 生产 CSP 无 `unsafe-eval`/`unsafe-inline`（script-src），含 `object-src 'none'`/`base-uri 'self'`/`frame-ancestors 'none'`；`withGlobalTauri` 未设置（未暴露全局 `__TAURI__`）。
- **路径穿越防护**：`read_log` 有 canonicalize + 前缀校验；`skill_set_enabled`/`skill_delete`/`skill_open` 经 `scan::resolve_managed`（canonicalize + 受管根前缀）与「身份声明 name 复验」；技能导入目标目录名取自 frontmatter `name` 并经 `is_valid_skill_name` 校验。
- **YAML 注入防护**：MCP 结构化字段全部经 `managed::yaml_quote`（单引号包裹 + `''` 转义，`managed.rs:144-146`，含专项测试 `:976-989`）；行 id 经 `validate_row_id` 白名单字符校验（`:130-141`）。
- **依赖必要性（除 ENG-02/03/04）**：`class-variance-authority`/`clsx`/`tailwind-merge` 均由 `components/ui/*` 与 `lib/utils.ts` 实际使用；`encoding_rs` 由 `text.rs` 中文解码使用；`serde_json` 9 处使用；`tauri-plugin-opener` 由 `StatusCard.tsx:472` + `SkillsPanel.tsx:205` 使用；`tauri-plugin-single-instance` 由 `lib.rs` 使用。
- **仓库卫生**：`dist-bundles/`（63MB）、`dist/`、`src-tauri/target/`、`*.tsbuildinfo` 均被 `.gitignore` 忽略且 `git ls-files` 零命中——无构建产物入库。

---

## 5. 整改实施记录（本轮）

> 本节记录按 §3 执行的实际改动（源文件已修改，非只读审查结果）。

### G0 · Tauri 2.11.5 源码级复核（已完成，结论见 §1 SEC-01）

- **如何具备条件**：本机 `cargo`/`rustc` 缺失、`~/.cargo` 已被删除 → 先安装 rustup（stable 1.98.1）+ 用 VS2022 BuildTools `vcvars64.bat` 导入 MSVC 环境；`cargo check --all-targets` 基线通过（零警告）后，registry 源码重新可用。
- **结论**：`tauri-2.11.5/src/webview/mod.rs:1819-1850` 的显式来源守卫（`!is_local` 分支）+ 官方测试 `:2423-2477`，证明**远程源页面无法调用应用自定义命令**。→ 原「闸门② ACL 侧不拦截」的推论被推翻，SEC-01 的 RCE 后果链**不成立**。两条 Windows IPC 路径的 `url` 来源均已核验（`protocol.rs:488-496` 的 `Origin` 头；`wry-0.55.1/src/webview2/mod.rs:896-910` 的 `args.Source`）。
- **对定级的影响**：**未擅自改定级**；SEC-01 最终等级待用户裁定（本次仅纠正事实）。

### G1 · 内嵌 Web GUI 窗口与 IPC 边界（已实施）

- **涉及文件**：`src-tauri/src/commands/dsh.rs`（+165/−5，含新增单测）
- **改动**：
  1. 新增 `is_loopback_host()` / `url_is_loopback()`（含 IPv6 `[::1]` 兼容）。
  2. `create_embedded_web_gui_window`：`on_navigation` 由「恒 `true`」改为「**仅回环 http(s) 放行**；非回环 http(s) 交系统默认浏览器打开并取消导航」；非 http(s)（`about:`/`data:`/`blob:`）保持放行，避免 dsh UI 内部行为回归。
  3. `probe_web_ready`：非回环 URL 直接返回 `false`（消除 SSRF 原语）。
  4. `create_web_gui_window`：非回环 URL 直接返回 `Err`（消除「任意源内嵌」）。
  5. 新增 `#[cfg(test)] mod loopback_url_tests`（回环接受 / 非回环拒绝 / IPv6 方括号 / `169.254.169.254` / `localhost.evil.com` 等）。
  6. **未实施**原 §3-G1 要点③（命令入口窗口守卫 / isolation pattern）：源码复核表明 `!is_local` 已覆盖，属冗余防御，按「不越界」纪律不实施。
- **验证结果**：
  - `cargo check --manifest-path src-tauri/Cargo.toml --all-targets` → **exit 0，零警告**。
  - `cargo test --lib` → **206 passed; 0 failed**（含新增 3 条 `loopback_url_tests` 全绿）。
  - `npx tsc --noEmit` → **exit 0**（未涉及 TS 改动，基线复核）。
- **未验证（受限）**：`cargo test` 的集成测试未逐个跑（部分依赖真实网络/dsh 环境，属 nightly 范畴）；手工 GUI 点击验证未做（无交互式桌面会话）。

### G3 · MCP/插件受管区块写入安全（**已实施**）

- **涉及文件**：`src-tauri/src/core/plugin/managed.rs`（`apply_body` + 新增单测）、`src-tauri/src/core/mcp/mod.rs`（`apply_with_verification`）、`src-tauri/tests/mcp_pipeline_test.rs`（新增端到端回归）
- **改动（对应 §1 SEC-03 的①②）**：
  1. `managed::apply_body`（**managed / shared / mcp 三个家族的唯一写入路径**）：写入前校验 `body` 不含本家族 `begin_marker`/`end_marker` 子串，命中即 `Err`、**不落盘**。抽样确认所有 `apply_*` 均汇入 `apply_body`。
  2. `mcp::apply_with_verification`：`fingerprint_outside(&after_content)?` 的 **`?` 早退** 改为把错误压入 `failures`（写后 fingerprint 改为 `Option`，与 `before` 用 `as_deref` 比较）→ 损坏路径也进入统一回滚。
- **未实施**：原 §3 附带的「泛化原子写（合并 `write_atomic` / `write_atomic_md`）」——那属 §2.2 的 P2 重复代码，不属 SEC-03；按「不越界」纪律不动。
- **验证结果**：
  - `cargo check --all-targets` → **exit 0，零警告**。
  - `cargo test --lib` → **210 passed**（新增 `写入体含_marker_时拒绝且不改动文件`、`正常写入体不受_marker_防护影响`）。
  - `cargo test --test mcp_pipeline_test` → **17 passed**（含新增 `g3_raw_config_with_managed_marker_is_rejected_with_zero_write`；原 `raw_config_channel_passes_official_fields_verbatim_including_js` 仍绿 → 合法 `!!js` 透传未被误伤）。
  - `plugin_pipeline_test` 5 / `skill_write_pipeline_test` 1 / `skill_import_pipeline_test` 4 均绿。

### G4 · 进程管理器锁中毒健壮性（**已实施**）

- **涉及文件**：`src-tauri/src/core/process.rs`
- **改动**：新增 `fn lock_or_recover<T>(&Mutex<T>) -> MutexGuard<T>`（`unwrap_or_else(|p| p.into_inner())`，与 `commands/config.rs`、`plugin/managed.rs`、`plugin/mod.rs` 既有约定一致）；**39 处** `.lock().unwrap()` 全部改为 `lock_or_recover(&…)`。`op_lock` 原本已是 `into_inner()`，未动。
- **验证结果**：
  - `cargo check --all-targets` → **exit 0，零警告**（无新增 lint）。
  - `cargo test --lib` → **210 passed**（新增 `锁中毒后仍可恢复读写`：注入持锁 panic 使互斥量中毒，断言仍可读写；`状态读写不panic`）。
  - 全仓仅剩 1 处字符串`.lock().unwrap()`，位于 `lock_or_recover` 的**文档注释**内（非代码）。

### 其余组（G2、G5–G8）状态

### G2 · commands 层错误契约与契约残留（**已实施**）

- **涉及文件**：`core/plugin/state.rs`、`commands/{plugin,mcp,skill,config}.rs`、`components/SkillsPanel.tsx`
- **改动**：
  1. 新增 `PluginError::ipc_message()`（`[<exit_code>] <message>`），删除 3 份逐字相同的 `format_error`（含 `skill.rs` 的 `#[allow(dead_code)]` 死函数）；9 个调用点改用 `e.ipc_message()`。与现有 `Display`（只给 `message`）刻意区分，注释已说明。
  2. 删除 `ConfigView.github_token` 字段与赋值（TS 已无此字段）。
  3. 删除 `pub type SkillEntryView`（零引用）与随之无用的 `PluginError`/`SkillEntry` 导入。
  4. `skill_forget_source`（后端命令 + TS 封装齐全但无 UI 入口）：在 `SkillsPanel` 已有的来源列表中加「移除记录」按钮（二次确认 + 明确提示“不删技能文件”），并导入 `Trash2`。选择“补 UI”而非“删命令”，因为 ADR-0008 已登记来源记录为产品能力，只删元数据不动技能文件是安全语义。
- **验证**：`cargo check --all-targets` 0 警告；`tsc --noEmit` 0；`cargo test --lib` 210 passed；`plugin_pipeline_test` 5 / `mcp_pipeline_test` 17 / `skill_*` 5 全绿。
- **副作用审查**：`ipc_message` 保持原有 `[code] msg` 前端可见格式（前端仅 `String(e)` 展示，未解析），无行为变化。

### G5 · 配置与日志可观测性（**已实施**）

- **涉及文件**：`commands/version.rs`、`core/config.rs`、`commands/config.rs`、`components/LogPanel.tsx`
- **改动**：
  1. **RT-02**：`list_npm_versions` 的 `serde_json::from_str(...).unwrap_or_default()` 改为具名 `Err`（附 200 字输出片段），消除“解析失败被误报为网络问题”。
  2. **RT-03**：新增 `AppConfig::load_checked() -> (Self, Option<LoadIssue>)`（`load()` 改为其包装，签名不变）；`get_config` 改用 `load_checked` 并把问题落 logger。`LoadIssue::{ParseFailed, TokenDecryptFailed}` 带中文 `message()`。
  3. **SEC-04（× ADR-0009 D5）**：`LogPanel` 新增「导出打码版」（`redactWebTokens`，仅替换 token 值）；**默认展示/落盘仍为明文**，严格遵循 D5 裁定。原 `exportLog` 与新增导出共用 `downloadText`（顺带消除 blob URL 处理重复）。
- **验证**：`cargo check --all-targets` 0 警告；`cargo test --lib` 210 passed；`tsc --noEmit` 0；`npm run build` ✓。
- **备选未做**：未将默认日志改为打码（与 ADR-0009 D5 冲突，需产品改判）。

### G6 · 技能路径校验与导入边界（**已实施**）

- **涉及文件**：`commands/logs.rs`、`core/skill/editor.rs`、`core/skill/sharing.rs`、`core/mcp/mod.rs`、`docs/adr/0008-*.md`
- **改动**：
  1. **SEC-05a**：`read_log` 改为读 `canonical_full`（已校验的规范化路径），消除 TOCTOU。
  2. **SEC-05b**：`open_skill_file` 改为打开 `canonicalize()` 后的路径（归属判定内部已规范化）。
  3. **RT-05**：`sharing.rs::detect` 的 `read_link().expect("已确认可读")` 改为一次 `if let Ok(target) = read_link(&view)`（同时消除两次调用间的 TOCTOU 与 panic 路径）；`mcp/mod.rs::set_state` 的 `row.expect(...)` 改为 `let Some(row) = row else { return Err(PluginError::internal(...)) }`。
  4. **SEC-06**：在 ADR-0008「安全边界」补记本地来源（`file://`/绝对路径/UNC）的安全含义与其不新增攻击面的前提（Tauri `!is_local` 拒绝远程源）。
- **验证**：`cargo check --all-targets` 0 警告；`cargo test --lib` 210 passed；`path_inject_test` 5 / `skill_write_pipeline_test` 1 / `mcp_pipeline_test` 17 全绿。
- **备注**：`detect` 的条件由 `symlink_metadata().is_ok() && read_link().is_ok()` 简化为 `read_link().is_ok()`，二者在 symlink 语义上等价（`read_link` 仅对链接成功）。

### G7 · 前端运行时与类型契约（**已实施**，含 TC-03）

- **涉及文件**：`components/AppShell.tsx`、`components/VersionPanel.tsx`、`components/LogPanel.tsx`、`hooks/useIsMobile.ts`、`lib/panel-layout.ts`、`core/github.rs`、`commands/version.rs`；**删除** `lib/version.ts`；**新增** `src-tauri/tests/contract_types_test.rs`
- **改动**：
  1. **RT-04**：`toggleMaximize()` 加显式 `.catch()`（`try/catch` 只能捕同步抛出，原 `void` 会成未处理拒绝）。
  2. **TC-02（顺序单一真相源）**：`github::cmp_semver_desc` 改为 `pub`；`list_npm_versions` 在 Rust 侧排序（npm registry 原始升序）；`VersionPanel` 不再重排；**删除** `lib/version.ts`（重复的 semver 实现，仅此一个消费者）；顺带去掉 `parseVersionSegments` 的无用 `export`。新增 Rust 单测 `test_cmp_semver_desc_npm_channel_order`。
  3. **死导出**：`useIsMobile` 改引 `MOBILE_MAX_WIDTH`（消除断点双源，CSS 副本已标注）；`SPLIT_PANEL_MIN_WIDTH` 去 `export`（仅模块内用）。
  4. **TC-03（契约快照测试，新文件）**：33 组 Rust DTO ↔ TS interface 字段集合对账（含 `rename_all` 转换），已入 `ci.yml`；非空洞性经“注入真实改名→FAILED→还原”实证（详见 §1 TC-03）。
- **验证**：`tsc --noEmit` 0；`npm run build` ✓（`index-*.js` 由 452.70→451.67 kB，删除重复模块的实测收益）；`cargo check --all-targets` 0 警告；`cargo test --lib` **211 passed**；`contract_types_test` **4 passed**。
- **纠错记录**：① `SPLIT_PANEL_MIN_WIDTH` 初次被误删（初稿 grep 排除了定义文件），`tsc` 立即报 `TS2552` → 按“仅去 export”恢复；② 契约测试首跑抳出 2 处自身书写错误（`panic!` 格式串、`lowercase` 输入形态）。

### G8 · 工程与 CI 卫生（**已实施**；ENG-05 决策为不签名）

- **涉及文件**：`.github/workflows/nightly.yml`、`src-tauri/Cargo.toml`、`package.json`/`package-lock.json`
- **改动**：
  1. **ENG-01**：`starting_convergence_e2e` 加入 `nightly.yml` 的 `$tests` 列表（带 `--ignored`）。**并勘误初稿**：该文件本就是 `#[ignore = "…"]`，非“无 ignore”（见 §1 ENG-01）。
  2. **ENG-02**：`@types/node` `^26.4.1` → `^22.20.2`，与 CI 的 Node 22 对齐。
  3. **ENG-04**：从**库**依赖移除 `Win32_Graphics_Gdi`（仅 tests 需要，dev-deps 保留）与 `Win32_System_Environment`（全仓零引用）；`icon_window_test` 仍绿。
- **撤销/回滚**：**ENG-03 为初稿误报**——`shadcn` 经 `src/index.css:3` 的 `@import "shadcn/tailwind.css"` 被实际使用，移除后 `vite build` 立即报 `Can't resolve 'shadcn/tailwind.css'`；**已恢复**并确认构建通过。已在 §1 ENG-03 记录勘误。
- **未实施（已决策收口）**：**ENG-05** —— 产品所有者决定**不购买证书、不签名**（接受 SmartScreen 告警）；已写入 §1 ENG-05（含未来若启用的备查配置），本项不再作为待办。另一项 `Win32_Console`/`Globalization` 等剩余 feature **已按引用保留**，未做进一步收窄（避免为微小收益引入构建回归风险）。
- **验证**：`cargo check --all-targets` 0 警告；`cargo test --lib` 211；全部 12 个 CI 集成测试通过；`icon_window_test` 1 passed；`check-version-sync` ✓ 0.9.1。

---

*报告生成：§1–§4 为只读审查；§5 为按 §3 执行的实际改动记录。*
