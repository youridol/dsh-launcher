# Changelog

## [0.9.6] - 2026-09-16

### 修复（依据 2026-09-16 dsh-v0.1.6-alpha.1 升级故障审计）

- 故障现象：升级 dsh 0.1.6-alpha.1 后（GitHub 通道），dsh 启动显示"已就绪"，但 Web UI
  的工作区与 Sessions 面板全部不可访问。dsh stderr 记
  `5 entries did not activate`：session-controller / workspace-controller / ui-git-graph /
  ui-task-board 等 pending（waiting for service: workspaceRegistry）。
  实测 RPC（带 token）返回 `gateway/service-unavailable: sessionController is unavailable`。
- 数据无损：sessions/（74 MB，session.v3.jsonl.zstd）与 storages/workspace.json
  （domain v2，schema 与 0.1.6 一致）全部完好；健康实例启动后 session/list 完整返回。
- 成因（证据链见当日审计记录）：切通道安装（pnpm install/build + 插件自动同步失败 2 个
  未阻断）与 dsh 启动期 profile 依赖闭环解析存在时序竞争；叠加启动器旧停止路径仅
  等待 1 秒即 taskkill /F 强杀，留下进程级残留（task-board ledger-v2.lock 实测残留，
  导致 ui-task-board `ledger is already owned by process <pid>`）。健康审查缺失使
  "端口监听 ≠ 服务健康"的失败实例被标为 Running。

### 变更（预防计划 P0/P1/P2 落地）

- **P0-1 启动健康审查（process.rs）**：`spawn_startup_probe` 在端口就绪后增加 3 秒
  宽限窗口扫描本次启动的 stderr 落盘文件（每次启动截断重建，故内容即本次输出），
  命中 dsh `auditStartupEntries` 固定措辞 `entries did not activate`（含 `): pending` /
  `): failed` / `failed to import` 明细）即升级为 Error 日志（附最多 16 行明细，可直接
  定位 pending 的服务）并**请求自动重启一次**（探活线程置请求位，对账线程消费执行；
  `pending_restart_done` 门闩保证只重试一次，重启后仍 pending 判定为持久性问题不再
  循环重启）。
- **P0-2 停止流程加固（process.rs）**：优雅等待 1s → 10s（300ms 轮询探测；dsh 的
  cordis shutdown waterfall 实测 3-8s）；优雅退出与强杀路径均补充分支日志；强杀/
  清剿后新增 `cleanup_stale_dsh_locks`：清理 `$DSH_HOME/task-board/ledger-v2.lock`
  等已知残留锁——仅当锁内宿主 pid 已死亡（或内容不可解析）才删，宿主仍活（疑似外部
  dsh 实例）则跳过并告警，不破坏活实例互斥；幂等，无锁文件零开销。
- **P1-1 安装后强制收敛（github.rs）**：GitHub 通道 clone+build 完成后、创建 shim 前，
  执行一次官方收敛通道 `dsh plugin install --profile web`（按 lockfile 重新
  pnpm install 并对账 dsh.profile.bundles，幂等），确保 profile 依赖闭包与新安装一致
  后再让用户启动，消除 pnpm 半状态竞态。失败只告警不阻断（dsh 本体已构建成功；启动
  健康审查 P0-1 会兜底检出 pending）。
- **P1-2 同步失败不静默、不带病重启（lib.rs / plugin/mod.rs）**：① 自动同步存在失败
  项时升级为 Error 日志并列出失败包名与处置指引（此前仅 Info"成功 0，失败 2"）；②
  `plugin::sync` 存在失败项时**跳过自动重启 dsh**（避免以半更新状态的 profile 启动
  dsh——审计案例诱因之一），提示重试失败项或执行「收敛」后手动启动。
- **P2 诊断透明化**：健康审查日志附带 did-not-activate 明细（服务名 + 等待原因），
  修复 dsh 0.1.6 中 workspace 行自身失败不出现在汇总列表、只能靠下游 pending 间接
  推断的诊断盲区。
- 新增单测 2 项：`parse_lock_holder_pid`（实测锁样本/无 pid/非数字/垃圾输入）、
  stderr 未激活匹配逻辑（现场样本 + 健康样本）。
- 验证：`cargo check --all-targets` 0 错误 0 警告；`cargo test --lib` 219/219 通过；
  `npx tsc --noEmit` 0 错误；`npm run build` 成功（451.68 KB / gzip 140.15 kB）；
  集成测试 plugin_pipeline / starting_convergence_e2e / config_default 全过。

## [0.9.5] - 2026-09-12

### 变更

- README：引言补一行「版本随意切、随意退」
- 在「dsh-launcher 就是把这些事搬进一个启动器和提供桌面窗口」的特性列表里，
- 于「点一下就能装」之后新增一条，突出用户最关心的版本管理能力：
- 版本管理面板把 npm 与 GitHub 两条通道的**历史版本全列出来**（最新置顶）；
- 想升就升、想退就退——点哪个装哪个，可从最新版一键**退回**到任意稳定版，或换通道再装；
- 不必手记 `npm i -g xxx@<版本>` 或自行 `git clone --branch <tag>` 再编译；
- 切版本/换通道不会动 ~/.dsh 的会话、技能与插件配置（指向下方「🔒」章节）。
- 依据（源码可复核）：VersionPanel 通过 listVersions("npm")/listVersions("github")
- 拉取两通道全量版本，installVersion(channel, version) 支持安装任意列出版本，
- 故「升级」与「退回」是同一入口。
- 验证：README 390 行；6 处配图引用；10 个代码围栏成对；8/8 <details> 配对。

## [0.9.4] - 2026-09-12

### 变更

- README：补充「换版本/换通道/卸载不碰 DSH_HOME 数据」说明
- 用户关心的核心问题：切官方版本时会不会动 ~/.dsh 的会话数据、技能与插件。
- 经源码核对后如实写入（非宣传口径）：
- 新增章节「🔒 换版本 / 换通道 / 卸载，都不会碰你的数据」：
- · 列出 DSH_HOME 里到底有什么（sessions/ 会话、settings.yaml / .credentials.yaml、
- storages/、task-board/、profiles/<name>/ 插件、~/.agents/skills/ 技能）；
- · 用表格对照「动作 / 动的部分 / 不碰的部分」四种情形
- （切通道、升降级、默认卸载、关掉保留开关的卸载）。
- 依据（源码/文档可复核）：
- · commands/version.rs 的通道切换只删程序本体（npm 全局包 或 github-dsh 源码目录 +
- 自家 dsh.cmd shim），注释明确“不清 DSH_HOME 数据”；
- · keep_dsh_home_on_uninstall 默认 true（core/config.rs:60）；
- · 关掉该开关时删前校验目录特征（含 profiles/ 或 settings.yaml）才删除。
- 同时诚实标注例外：插件装在 profiles/ 下与 dsh 版本无关，但与新版不兼容时启动器
- 只会把冒头那一行**禁用（可逆）**并提示，不会卸载（核验：handle_boot_failure 中
- uninstall 调用数为 0）。
- 同步补进「功能一览」版本管理一行与 FAQ（新增一条，便于直接命中搜索）。
- 验证：README 386 行，6 处配图引用，8/8 <details> 配对，10 个代码围栏成对。

## [0.9.3] - 2026-09-12

### 变更

- README 全链路重写：大白话介绍 + 5 张界面配图 + 事实校正
- 面向首次接触的用户重写 README，并修正原文档中的过时/错误陈述。
- 新增配图（png/1-5.png，均为 1600x900）
- 1 主界面（版本管理 + 工具链 + 实时日志）
- 2 MCP server 管理    3 插件管理
- 4 技能管理           5 设置
- 每张均在对应章节配一段"这张图能做什么"的说明，而非只堆图。
- 可读性
- 新增「这是个什么东西？（大白话）」：先讲清 dsh 是什么、launcher 解决什么痛点，
- 并显式声明"它不做什么"（不改 dsh 源码 / 不接管 DSH_HOME / 不偷偷安装）。
- 新增「快速开始」（装 3 步 + 用 5 步典型流程）、「FAQ」（404 / allowBuilds /
- 内嵌打开拿不到 token / 状态卡启动中 / 是否误杀进程 / 数据是否被改动）。
- 功能清单改为表格；命令行与长说明折叠为 <details>；顶部加徽章行（含 dsh-plugin 话题）。
- 事实校正（原文与代码/ADR 不一致处）
- 「技能共享：以 ~/.agents/agent 为唯一真源…链接模式」→ 与实际不符：
- 共享真源是 agentsHome 根（~/.agents），且 link/config 两种共享模式**已退役**
- （CONTEXT「技能共享（已退役）」）。改为描述真正的技能管理能力
- （启停 = disable-model-invocation、.trash 可恢复删除、批量导入、手动检查更新）。
- ADR 范围「ADR-0001~0005」→ 实际为 ADR-0001 ~ 0009。
- 命令行示例 `skill list` 不存在（实际为 status|apply|migrate|repair-links）→ 修正并加注说明。
- 补充 mcp CLI 段落与 --resource 参数。
- 进程模型措辞精确化：无窗口启动（CREATE_NO_WINDOW）、stdout/stderr 落盘后 tail、
- taskkill /PID /T（SIGTERM 语义，约 1 秒）→ /F 强杀 → 端口兜底清剿（清剿前校验身份防误杀）。
- IPC 命令数经 generate_handler! 权威核对为 48（非估算值）。
- 仓库元数据
- topics 修正：移除不准确的 `electron`（本项目为 Tauri 2，DESIGN §2 明确"抛弃 Electron"），
- 补充高精准度话题 dsh-launcher / cordis / cordis-plugin / tauri-app / desktop。
- 保留既有的 `dsh-plugin`（已确认本仓库出现在该话题搜索结果中）。
- 验证：5 张配图与全部本地链接（CONTEXT/CHANGELOG/LICENSE/docs）均存在；
- 外部链接（deepseek-harness / topics/dsh-plugin / releases / tauri.app）均 200；代码块配对（10 个围栏）。

## [0.9.2] - 2026-09-12

### 变更

- 修复插件安装的 4 个 BUG（诊断缺失 / 无谓重启 / GitHub URL 误分类 / 镜像未注入）
- 依据用户报告「输入 dsh-market 装插件报错：[5] 插件安装失败（已回滚）: 命令退出码: 1」
- 所做的严格模式排查与修复。已实测：dsh-market 不是 npm 包（真实包名为 dshmarket）；
- 用户给出的 https://github.com/dsh-market/dsh-market 是**有效**的 git 源。
- BUG-1（P1）安装失败原因被丢弃，用户无法自助定位
- 关键实测事实：pnpm 把致命错误**写在 stdout 而非 stderr**（pnpm 11.24 下 stderr 为 0 行）。
- 旧代码只取 stderr 尾部 → 等于什么都没抓到，只剩「命令退出码: 1」。
- stream.rs 新增 run_streamed_checked + 结构化 StreamFailure：
- 同时捕获 stdout/stderr 尾部（上限 12 → 40 行，12 行会让真实原因被后期噪声挤掉）；
- 过滤 `dsh:` 自身噪声行；run_streamed 保持原 API（委托 + message()）。
- plugin/mod.rs 新增 describe_pnpm_failure：对 ERR_PNPM_FETCH_404 与
- ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED 给出指向性提示；未识别情形原样透传（不猜测）。
- E2E 验证：错误信息现包含 pnpm 原始报错 + 确切 allowBuilds 键名 + 操作指引。
- BUG-2（P2）安装失败回滚后仍重启 dsh（无谓 + 误导）
- rollback_after_failed_official_op 改为返回「是否完全还原」；
- 完全还原则**不重启**（避免中断会话、避免日志出现「dsh 已启动」造成成功错觉），
- 仅回滚未完全还原时才重启以对齐磁盘状态。
- BUG-3（P2）GitHub 裸 URL 被误分类为 Unknown
- https://github.com/<owner>/<repo>（不带 .git）此前落到 Unknown →
- 不参与 upstream 自动同步，且无法从 spec 推断包名。
- spec.rs 新增 looks_like_hosted_git_url：识别 GitHub/GitLab/Bitbucket 的
- owner/repo 两段 URL；仅限已知托管站且恰两段（避免 releases/download/*.tgz 误判）。
- 实证：pnpm 会把它规范化为 github:owner/repo。
- BUG-4（P2）插件操作未注入 npm 镜像源
- 按**官方开放机制**注入：官方 dsh plugin 是 pnpm 的薄转发器
- （apps/cli/src/plugin.ts:120-163，args 原样透传），故追加 pnpm 原生 --registry。
- 仅对访问 registry 的操作注入（纯 git/路径/tarball 依赖不注入），
- 不触碰 profile 的 .npmrc / pnpm-workspace.yaml（不违反 ADR-0005 白名单）。
- UI（防再犯）
- PluginsPanel：标签/placeholder 明确「npm 包名 / GitHub URL」，
- 并直接点明 dshmarket ≠ dsh-market（行 id），给出 URL 形态与钉 commit 建议。
- 按产品决策 A：不做「一键 allowBuilds」—— 保持与官方 dsh 一致
- （pnpm ≥10 拦构建脚本需用户按提示自行 allowlist），启动器不写 profile 文件。
- 验证：cargo check --all-targets 0 警告；cargo test --lib 217 passed（+7 新增）；
- plugin_pipeline_test 5 / mcp_pipeline_test 17 / contract_types_test 4 全绿；
- tsc + vite build 通过。E2E 均以隔离 DSH_HOME 执行，用户真实 profile 未被改动。

## [0.9.1] - 2026-09-11

> 本版为**「启动后未就绪」BUG 修复**。硬边界：未修改 `deepseek-harness` 任何代码；
> **未引入新依赖**；**未新增 npm 依赖**；**未新增 Tauri capability 权限**。

### 修复

- **dsh 冷启动超过 8 秒后状态永久卡「启动中」，导致内嵌打开失败（核心 BUG）**。
  `Starting → Running` 此前**只有一条**路径：启动探活线程，硬上限 8 秒；超时即打日志
  后 `return`，**再无任何后续收敛尝试**。而本机实测冷启动「端口监听 at 8.83s」
  （`dsh-cost-meter` 插件 + MCP filesystem 拉长预热）已越过该上限，于是：
  - 「内嵌打开」阶段③在 25 秒内始终读不到 `running` → 报
    `dsh 启动后未就绪（端口未监听）`（**端口其实已在监听**，文案也是错的）；
  - 状态永久停留 `Starting`（UI 上等同"运行中"，端口输入框被禁用）；
  - 5 秒对账线程的收养分支只认 `Stopped` → **手动启动的 dsh 也无法被接管**，
    这正是"手动启动后内嵌打开按钮同样失败"的成因。
  修复：
  - **对账线程新增 `Starting` 分支**（无上限持续探测）：端口就绪 → `Running`；
    进程已退出 → `Stopped`（仍由探活线程做插件归因，ADR-0005 D12 语义不变）。
    收敛判据区分监听者归属 —— `Managed`（本托管 pid）/ `Adopted`（命令行形如 dsh）
    可认定就绪，`Foreign`（无关进程占端口）**绝不**判定就绪（防误判 → 防停止时误杀）。
  - **`start_dsh` 观测到端口监听即置位**：该命令本就轮询到端口已在监听，此前却只用它
    拼返回字符串、不推进状态（观测到的事实被丢弃）；现在按需调用一次对账。
  - **收养允许从 `Starting` 进入**（此前只认 `Stopped`），解开"手动启动无法接管"的死结。
  - 启动探活线程 8 秒超时由 `WARN` 降为 `INFO`：它是"冷启动偏慢"而非失败。
- **外部启动的 dsh 复用陈旧 token，导致内嵌打开空转 40 秒**。
  dsh 的访问 token 是**进程级随机数**（`packages/client/connection/src/browser-auth.ts`
  的 `processLaunchToken`），仅从该进程 stdout 打印。收养时旧实现**无条件沿用**缓存地址：
  若缓存来自上一轮进程（外部实例的 token 启动器原理上拿不到），前端会拿这段失效 token
  反复探测失败 → 空转 40 秒后报"访问地址无效"。现在收养时**先用 HTTP 探测验证缓存对
  当前监听者仍有效**，失效即同时清除内存与磁盘缓存（`last-web-url`）。
- **「启动时自动打开 Web GUI」/桌面快捷方式在 8 秒内未监听即放弃**：上限放宽到 30 秒，
  并在探测到监听时顺带对账，覆盖超过 8 秒的冷启动。

### 新增

- **「接管并重启」失败引导**：当内嵌打开因"外部实例无 token"失败时，弹窗不再只报一句
  无从下手的错误，而是说明原因并提供**接管**按钮 —— 停止该外部实例并由启动器重新拉起，
  使 token 可被捕获。因会中断该实例上的会话，故**由用户显式点击**触发。
- 新命令 `is_dsh_managed`（区分托管/外部实例）与 `take_over_dsh`（接管）。
- 失败文案按**真实原因**分流：进程已退出 / 仍在启动未就绪 / 已监听但 HTTP 未就绪 /
  外部实例无 token —— 不再一律误报"端口未监听"。
- 测试隔离缝 `DSH_LAUNCHER_DATA_DIR`（仅集成测试使用；正常运行时该变量不存在，
  行为与原先完全一致）：端到端用例可把日志与 token 缓存隔离到临时目录，
  **不触碰用户正在运行的启动器文件**。
- 回归测试：`starting_convergence` 纯函数语义（进程存活但端口未就绪**必须**保持
  非终态）、`Listener::is_owned` 归属判定、`is_managed` 初值；
  以及隔离式端到端用例 `tests/starting_convergence_e2e.rs`
  （真实冷启动收敛 + 收养不得复用陈旧 token）。

### 严格模式审计整改（全链路只读审查 → 逐项修复）

> 依据 `docs/AUDIT_REPORT_STRICT.md`（P0×1 / P1×3 / P2×18，已逐项处置）。
> 硬边界同本版上方：未修改 `deepseek-harness` 任何代码；**未引入运行时新依赖**
> （新增仅限 dev/测试用途）；**未新增 Tauri capability 权限**。

#### 安全 / 数据完整性

- **受管区块写入可被写坏且回滚被绕过（P1）**：`plugin::managed::apply_body`（managed /
  shared / mcp 三个家族的**唯一**写入路径）新增 marker 子串校验——写入体含本家族
  marker 即拒绝且**零落盘**；同时修复 `mcp::apply_with_verification` 中
  `fingerprint_outside(...)?` 的 **`?` 早退**（写后区块损坏时会在进入回滚前返回，
  导致坏文件永久残留），改为压入失败列表走统一回滚。
- **IPC 接受前端任意 URL（P1）**：`probe_web_ready` 与 `create_web_gui_window` 增加
  **回环（127.0.0.1 / localhost / ::1）http(s) 校验**，消除 SSRF 原语与「任意源内嵌」。
- **内嵌 Web GUI 窗口任意源导航（P0→P1）**：`on_navigation` 由「恒 `true`」收紧为
  **仅回环放行**，非回环 http(s) 交系统默认浏览器打开并取消导航（与 `on_new_window`
  的外链分流语义一致）；非 http(s)（`about:`/`data:`）保持放行以避免 UI 回归。

#### 健壮性 / 可观测性

- **锁中毒即 panic（P1）**：`ProcessManager` 的 **39 处** `.lock().unwrap()` 统一改为
  容忍中毒的 `lock_or_recover`（与仓库其它模块既有约定一致）；此前任一线程持锁期 panic
  会连锁使后台监视线程死亡、状态永久不再收敛。
- **路径校验与实际使用不一致（TOCTOU）**：`read_log` 改为读取**已校验的规范化路径**；
  `open_skill_file` 改为打开 `canonicalize()` 后的路径。
- **生产路径 `expect`**：`skill::sharing::detect` 的 `read_link().expect(..)`（两次调用
  间存在 TOCTOU）与 `mcp::set_state` 的 `row.expect(..)` 均改为显式错误分支。
- **静默失败**：`npm view` 输出解析失败不再被 `unwrap_or_default()` 吞成空列表
  （此前会被上层误报为「网络/镜像源问题」）；新增 `AppConfig::load_checked()`，
  把「配置解析失败」「Token 解密失败」这类**此前静默**的回退事件落日志。
- **查询语义修正**：`npm` 通道版本列表改在 **Rust 侧排序**（registry 原始为升序），
  与 GitHub 通道统一。

#### 类型契约

- **新增前后端 DTO 字段集合契约测试** `tests/contract_types_test.rs`（33 组对账，含
  `rename_all` 转换；已入 CI，并用「注入真实改名→失败→还原」实证非空洞）。
- **删除重复实现**：移除前端 `lib/version.ts`（与 Rust 各写一份 semver 比较，有漂移风险），
  排序收敛到 **Rust 单点**；收紧若干无用 `export`。

#### 工程与发布

- **发布构建陷阱修复**：补上标准 `[features] custom-protocol`（`tauri::is_dev()` 即
  `!cfg!(feature = "custom-protocol")`），并在 README 明记「发布必须用
  `npm run tauri build`」——此前裸 `cargo build --release` 会产出 **dev 模式二进制**，
  启动即报 `ERR_CONNECTION_REFUSED`（本仓历史上一直缺该声明）。
- `starting_convergence_e2e` 纳入 nightly（`--ignored`）；`@types/node` 对齐 CI 的 Node 22；
  移除未使用的 `windows` crate feature（`Win32_System_Environment`、库依赖中的
  `Win32_Graphics_Gdi`，测试所需项保留在 dev-dependencies）。
- **发布可验证性**：Release 新增 `SHA256SUMS.txt`（安装包 + 便携包的 SHA-256，
  标准 `sha256sum -c` 格式）。本版**未做代码签名**（产品决策，接受 SmartScreen 告警）。

#### UI

- 技能管理面板：「共享资源（agentsHome）」移至**顶部第二行**（原在置底）；
  移除面板底部的「备份目录 / `.trash`」说明块（删除的 `.trash` 可恢复语义仍在删除确认弹窗中保留）。
- 日志面板：新增「导出打码版」（`token=` 值打码；默认展示与落盘仍为明文口径，
  遵 ADR-0009 D5 的产品决策）。

## [0.9.0] - 2026-09-11

> 本版为**技能导入、手动检查更新与外部打开（ADR-0008）**。硬边界：未修改
> `deepseek-harness` 任何代码；**未引入新依赖**（系统默认打开复用既有 `windows` crate 的
> `ShellExecuteW`）；**未新增 npm 依赖**；**未新增 Tauri capability 权限**（编辑打开走
> Rust 命令 + 闭集枚举，前端无法传任意路径）。

### 新增

- **批量导入 github 仓库（ADR-0008 D1/D4）**：批量管理面板 —— 填写<strong>仓库名</strong>
  （可选标签）+ <strong>仓库 URL</strong>，点「增加」追加到待导入列表，可一次添加多个
  仓库，点「确定」批量导入（另有「预览」只读、「移除」单条、「清空」列表）。每条：
  浅克隆 → **递归收集所有 `SKILL.md`** → 逐个校验（`name`/`description` 必需、`name`
  满足官方 `^[a-z0-9]+(?:-[a-z0-9]+)*$`）→ **扁平化**到 `<agentsHome>/skills/<name>/`。
  **单条失败不中断其余条目**，逐条返回结果。仓库名落进来源记录作展示标签。
  - 官方发现规则**只认扫描根顶层一层**（README「nested `**/SKILL.md` deliberately
    not discovered」），而真实技能仓库普遍分类嵌套（`skills/<分类>/<name>/SKILL.md`），
    故「克隆即用」会得到零个技能 —— 必须由导入器压平。
  - 目标目录名取 frontmatter 的 `name`，不取仓库目录名（本机 93 技能中 2 例目录名与
    name 不符）。
  - **整目录复制**（含兄弟资源文件与 `agents/` 子目录）—— 技能是目录，只复制 `SKILL.md`
    会断引用。
  - **文件级覆盖（Q29 A）**：上游有则覆盖、本地独有保留、上游删除不落地；应用前有
    「预览清单」需二次确认。
- **手动检查更新（ADR-0008 D5/D6）**：对来源注册表里的 URL 逐文件比内容，产出三类清单
  （新增 / 覆盖更新 / 本地独有保留）。**无定时任务、永不自动写盘**，应用需逐个确认。
- **来源注册表 `skill-sources.json`**（schemaVersion 1）：记录「哪个技能来自哪个 URL 的
  哪个 commit」，供 UI 显示来源与检查更新。**不叫 `skills.json`**（该名已被退役的
  ADR-0005 共享模块占用）。
- **共享资源编辑入口**：面板新增 `AGENTS.md` / `CONTEXT.md` 编辑按钮 + 技能根目录按钮。
  文件不存在时自动按模板创建。
- **每条技能新增「打开」按钮**（补 v0.8.0 遗漏）：用外部编辑器 / 系统默认程序打开
  `SKILL.md`。
- **可配置外部编辑器**：`AppConfig.editor_command`（空 = 系统默认程序），自由文本、
  引号感知切分，支持 `code --wait`；首次点击编辑弹一次性引导（选「系统默认」或
  「指定编辑器」）。

### 修复

- **★ `cargo test --lib` 测试二进制无法加载（P0，`STATUS_ENTRYPOINT_NOT_FOUND`）**：
  - **[问题定位]** 根因是 `Logger` 结构体直接持有 `tauri::AppHandle` 字段 —— 任何
    `#[cfg(test)]` 代码构造 `Logger`（本版 `import.rs` 的测试首次这么做）就会把整个
    `tao`/`wry` GUI DLL 栈（`user32`/`gdi32`/`comctl32`/`dwmapi`/`shcore`/`uxtheme`/
    `ole32`/`oleaut32`）链入 unittest 二进制；而该二进制没有 side-by-side manifest，
    加载到 v5 的 `comctl32.dll`，缺少 `SetWindowSubclass`/`TaskDialogIndirect` 等 v6
    导出 → 进程启动即失败。
  - **[关键日志证据]** PE 导入表对比：未改动 HEAD 的 lib 测试二进制只导入 13 个非 GUI
    DLL；引入 `import.rs` 后多出 9 个 GUI DLL。独立 worktree 检出 HEAD → 139 项测试通过，
    排除环境因素。
  - **[解决方案设计]** 把 `Logger.emitter` 类型擦除为
    `Box<dyn Fn(&LogEvent) + Send + Sync>`（`LogEvent = Line(LogLine) | Progress(ProgressPayload)`），
    由 `set_emitter` 在启动时把 `app.emit(...)` 封装进闭包；测试二进制里 `set_emitter`
    是死代码被链接器剥离，`tauri::AppHandle` 与 GUI 栈不再进入测试二进制。
  - **回归**：`cargo test --lib` 从「进程无法加载」恢复为 **193 项全通过**。
- **删除 dead code**：`core/events.rs` 的 `emit()`（进度事件推送唯一调用方已改为擦除后的
  emitter，该函数不再被引用）。

### 变更

- **`core/github.rs`**：抽出 `git_clone_command`（任意仓库浅克隆，复用 git 可执行解析 +
  Token 注入 + 镜像重写）与 `resolve_repo_url`（镜像**只对 `https://github.com/` 生效**）。
- **`core/config.rs`**：新增 `editor_command` / `editor_prompt_seen`（结构体带
  `#[serde(default)]`，旧 `config.json` 无需迁移）。
- **`Cargo.toml`**：既有 `windows` crate 增加 `Win32_UI_Shell` feature（`ShellExecuteW`；
  **非新依赖**，只是既有依赖的 feature 扩充）。
- **`core/skill/` 拆出新模块**：`import.rs`（递归扁平化 + 文件级覆盖）、`source.rs`
  （来源注册表）、`update.rs`（手动检查更新）、`editor.rs`（闭集目标 + 可配置编辑器 +
  `ShellExecuteW`）。
- **`docs/adr/0008-skill-import-update-and-open.md`**：记录 D1–D13（含 D10 类型擦除的
  完整工程约束）。
- **CI**：新增运行 `skill_import_pipeline_test`（hermetic 导入端到端）。

### 全链路审计修复（ADR-0009）

> 对 v0.9.0 基线执行的全链路只读静态审计（前端 / Rust / Tauri / 依赖 / 工程配置 / 测试 / 文档）
> 所发现问题的修复。逐条决策、精确契约与验收证据见 `docs/adr/0009-audit-remediation-plan.md`。
> 硬边界：**未修改 `deepseek-harness` 任何代码**；**未新增依赖**（并移除已成孤儿的 `tokio`
> dev-dependency）；**未放宽任何 Tauri capability**。

- **发版链路解阻（P1）**：`package-lock.json` 版本由 `0.7.1` 对齐到 `0.9.0` —— 该漂移会让
  `scripts/bump-version.mjs` 的一致性保护直接 `exit(1)`，**阻断任何一次发布**。新增
  `scripts/check-version-sync.mjs`（五文件六落点）接入 CI 与 Release 门禁，并新增
  `tests/version_sync_test.rs` 作为**独立实现**的防回归（避免脚本自身写错时两侧同时放过）。
- **安全（CSP）**：`tauri.conf.json` 的 `"csp": null`（完全禁用）改为最小策略，并单独配置
  `devCsp`（Tauri 在 dev 下取 `devCsp`、为空则回退 `csp`，只配 `csp` 会让 `tauri dev` 被
  Vite HMR 拦坏）。策略逐项按**实测取证**：`style-src` 必须含 `'unsafe-inline'`（产物存在
  运行时注入 `<style>` 的代码，否则 toast 样式静默丢失）、`connect-src` 含
  `ipc: http://ipc.localhost`、字体与图片走 `'self'` + `data:`、生产不需 `unsafe-eval`。
  新增 `scripts/verify-csp.py` 用真实产物 + 真实配置在 Chromium 中断言**零 CSP 违规**。
- **token 日志口径定案（D5）**：维持 v0.5.6 起的**明文**口径（产品决策：用户需从日志复制
  完整带 token 地址在外部浏览器打开 —— 裸 URL 会被 dsh 以 401 拒绝）。删除自 v0.5.6 起已无
  调用方的 `redact_web_token()` 及其单测，并把口径集中记录于 `core/logging.rs`；
  同步订正本 CHANGELOG v0.4.13 条目中与实现相反的「日志打码」表述。
- **前端（D7–D10、D18–D19）**：10 处 Tauri 事件订阅统一收敛到 `useTauriEvent` /
  `useRefreshOnEvent`，消除 7 处「cleanup 早于 `listen()` resolve → 监听器永不释放」的竞态
  及 `.then()` 缺 `.catch` 的未处理拒绝；移动端首屏不再被全屏日志面板遮挡（此前
  `rightOpen` 初值为 true 且 ≤640px 下右栏是 `inset:0` overlay）；git **卸载**不再误报
  「请在 UAC 弹窗确认后重新检测」（该路径实为 `-Wait` 同步）；日志文件快速切换加单调序号
  守卫（消除「标题是 B、正文是 A」）；10 处纯图标按钮补 `aria-label`；清理死代码
  （`setWebGuiIcon` 与对应 Rust 命令及其 IPC 注册、`AppConfig.githubToken`、`cn-toast`、
  重复 CSS 规则、`"use client"` 遗留指令）。
- **测试体系（D11–D13）**：4 个「只测测试文件内自行复刻的逻辑、零生产覆盖」的假测试改为
  **直调生产函数** —— 新增 `github_channel_state_test`（取代只测私有 `cleanup_dir` 的旧文件）、
  重写 `path_inject_test`（原用例构造 `Command` 后从不执行）与 `concurrency_test`
  （原测 tokio 运行时行为、且含 flaky 挂钟阈值），从 `commands/dsh.rs` 抽取生产纯函数
  `rgba_to_bgra_and_mask` 并补 6 项单测；`part_b_compliance_test` 的符号链接失败改为
  **跳过**而非 panic（否则整个文件在无开发者模式的 CI runner 不可用），并补 2 项失败路径
  契约测试；CI 集成清单由 8 个文件扩为 **12 个**（补入此前遗漏的 `plugin_pipeline_test`、
  `mcp_pipeline_test`、`part_b_compliance_test`、`version_sync_test`）；新增 `nightly.yml`
  以 `--ignored` 覆盖 3 个真实网络/环境集成测试（此前从未被任何流水线执行）。
- **文档（D14/D15）**：**重建** `docs/adr/0006-mcp-server-management.md` —— 该文件此前缺失，
  却已被全仓 **19 处**按 `§章节` / `D编号` 引用（MCP 的架构决策无从追溯）；重写
  `docs/DESIGN.md` 的模块划分对齐 `src-tauri/src/` 实际结构（此前列出的 `core/install.rs`、
  `core/mirror.rs`、`core/backup.rs` 等**并不存在**，且未收录 `plugin/`、`mcp/`、`skill/` 三棵
  子树）；`ADR-0004` 的版本同步「三处」订正为**五处**。
- **供应链与脚本（D16/D17）**：CI / Release / Nightly 的**全部** action 由移动标签
  （`@v4`、`@v2`、`@v0`、`@stable`）pin 到 **40 位 commit SHA**（SHA 经 `git ls-remote` 与
  GitHub API 双源核验），其中 `dtolnay/rust-toolchain` 由分支引用改为 `master` SHA 并
  **显式传 `toolchain: stable`**（该 action 由 `@rev` 推断工具链，pin 后必须显式指定）；
  `scripts/e2e-adr0006.ps1` 的三处本机绝对路径改为参数化入参 + 存在性校验，
  并移除「按启动时间扫射 `node` 进程」的误杀风险（改为对本脚本 PID 执行 `taskkill /T`）。

### 验证

- `cargo test --lib` **198 项全通过**（v0.8.0 的 168 项 → v0.9.0 新增 import/source/update/
  editor 单测，审计修复再补图标像素转换 6 项与死代码清理）。
- 集成测试 **12 个文件 / 63 项全通过**：`config_default`(2) · `tray_icon`(2) ·
  `concurrency`(7) · `icon_window`(1) · `path_inject`(5) · `github_channel_state`(2) ·
  `version_sync`(1) · `part_b_compliance`(17) · `plugin_pipeline`(5) · `mcp_pipeline`(16) ·
  `skill_write_pipeline`(1) · `skill_import_pipeline`(4)。
- `tsc --noEmit` / `tsc -p tsconfig.node.json --noEmit` / `vite build` /
  `cargo check --all-targets`（**零警告**）/ `cargo build --release` 全部通过。
- 端到端与专项门禁：`scripts/check-version-sync.mjs`（版本一致）、
  `scripts/verify-layout.py`（33 项布局断言）、`scripts/verify-csp.py`（8 项 CSP 断言）、
  `scripts/e2e-adr0006.ps1`（真实 dsh + 隔离 DSH_HOME，需显式提供路径参数）。


### 已知边界（诚实声明）

- **覆盖式导入会覆盖用户已改的技能内容**（文件级，本地独有保留但不做内容 diff 合并），
  且**导入/更新整目录覆盖当前不逐文件备份**（ADR-0007 的备份只在启停写盘路径）。UI
  文案已提示，应用前有预览清单需确认。
- 检查更新每次都浅克隆，多来源/大仓库较慢（180s 超时上限）。
- `skills.json`（已退役）与 `skill-sources.json`（新）两个文件并存，名字相近易混淆，
  已在前者退役、文档与代码注释反复标注。

## [0.8.0] - 2026-09-11

> 本版为**技能管理（ADR-0007）**：对用户级技能根提供技能列出、启用/停用、可恢复删除
> 与定位。硬边界：未修改 `deepseek-harness` 任何代码；**未引入新依赖**（Rust 侧无 YAML
> 库）；**未新增 Tauri 权限**（定位复用已授予的 `opener:default`）；未新增 npm 依赖。

### 新增

- **技能管理面板（ADR-0007）**：底栏「技能」入口由「技能共享」改为「技能管理」，
  不再分 Tab。列出官方两个**用户级**技能根 —— `<DSH_HOME>/skills`（官方 rank 400，
  跳过 `.system`）与 `<agentsHome>/skills`（官方 rank 500），共 6 个官方扫描根中的
  这 2 个。
- **单一滑动开关**：写技能文件 frontmatter 的 `disable-model-invocation`
  （官方契约：**缺省即允许**，仅显式 `true` 关闭模型面 → 「启用」= 删键、「停用」= 写
  `true`）。因此从未动过的技能**字节零变化**。面板明写「停用后仍可用 `/技能名` 手动调用」，
  因为官方语义里停用只关闭「模型面」，用户面仍开放。
- **可恢复删除**：移入所属根的 `.trash/<名称>-<时间戳>/`，不真正删除；**符号链接技能
  硬拒绝**（移动链接会把链接目标移走），前端同时隐藏该按钮。
- **rank 生效标注**：复刻官方 rank 常量（100/200/300/400/500/600，**不自创优先级**），
  同名技能中 rank 大者生效；被覆盖者**仍列出**但默认折叠并提示「停用它对模型无影响」。
- **定位按钮**：在资源管理器中定位技能文件（复用 `opener:default` 已允许的
  `reveal_item_in_dir`）。
- **搜索**：按技能名称或描述过滤。
- **`<agentsHome>/CONTEXT.md` 路径 helper**（`dshhome::agents_home_context_md`），为
  v0.9.0 的编辑入口预留。

### 修复

- **`disable-model-invocation` 语义正确性（P0，写入器实现缺陷）**：初始实现把「启用」
  写成 `disable-model-invocation: false` 而非**删键**，导致「停用 → 启用」无法逐字节回到
  原状。由单测 `启用为_true_删键且可逆回到原字节` 捕获并修复；现「启用」严格删除该行
  （只删该行自身的行尾符，绝不多删后续空行）。
- **前后端类型漂移（3 处，历史遗留）**：TS `ResourceState` 缺少 Rust 已有的 `"native"`
  变体（导致本机正常的「原生覆盖」状态被 UI 误显为「未建立」）、`ResourceStatus` 缺
  `needsLink`、`SkillStatus` 缺 `agentsSkillsRoot`。本版随共享 UI 一并退役，改由新的
  技能管理类型面取代。

### 变更

- **ADR-0005 的技能共享前端入口退役**：移除 `skillStatus`/`skillApply`/`skillMigrate`
  三个 IPC 命令与其前端封装、以及整个共享（link/config/迁移）交互界面。**Rust 后端保留**
  （`core::skill::sharing`）供 CLI `skill status|apply|migrate|repair-links` 使用，作为
  应急路径 —— 技能走官方 rank 500 原生覆盖，本不需要链接。
- **`core/skill.rs` 拆分为 `core/skill/` 模块**：`frontmatter.rs`（纯函数逐行外科手术）、
  `scan.rs`（只读扫描 + rank 标注）、`manage.rs`（身份校验/备份/原子写/复验/回滚）、
  `sharing.rs`（原共享模块，原样保留）。
- **恢复 `docs/adr/`**：`docs/DESIGN.md` 与 ADR-0001…0005 在 v0.7.0（`02ebf6a`）被整体
  删除，本版从 git 历史取回重建；新增 **`docs/adr/0007-skill-management.md`**。
- **`CONTEXT.md` 词汇表**：新增「技能管理（ADR-0007）」节（技能根 / 模型可调用 /
  用户可调用 / 停用 / 技能启用状态 / 生效与被同名覆盖 / 技能来源 / 导入 / 来源记录），
  并把原「技能共享」标记为退役。
- **CI**：新增运行 `skill_write_pipeline_test`（hermetic 端到端写操作测试）。

### 安全与正确性设计（写入器）

技能内容此前**从未**被本产品写入过 —— 这是首次引入「改写用户 markdown」的写操作类别
（ADR-0007 D2），故护栏是本版的核心交付：

- **逐行外科手术**：只改目标键那一行，其余字节**逐字节保留**（CRLF、BOM 缺失状态、
  无末尾换行、块标量 `>-`/`>`、纯多行标量、嵌套 `metadata:`、行内注释、非 ASCII）。
- **插入点固定为闭合 `---` 之前、第 0 列**：真实数据存在块标量与纯多行标量，插在
  `description:` 之后会落进标量体并静默改变语义。
- **歧义一律拒绝**（零猜测、零规范化）：无 frontmatter / 未闭合 / 键重复 / 值非裸
  `true`/`false`（`yes`、`"true"`、`1` 等）全部拒绝并给出具名原因；只写官方规范
  kebab-case 键（写 `disableModelInvocation` 会让官方**丢弃整个技能**）。
- **身份键 = 绝对路径**：写前重扫受管根，要求「路径仍在受管根内 ∧ 磁盘 frontmatter 的
  `name` 与面板声明一致」，不符即拒绝并提示刷新（防「面板打开后技能被改名，开关误伤
  同名技能」）；拒绝发生在读取目标文件之前。
- **备份 + 复验 + 回滚**：写前备份原字节到
  `%LOCALAPPDATA%\dsh-launcher\backups\skills\<名称>\<时间戳>\`；写后复验
  （frontmatter 仍可解析 ∧ `name`/`description` 仍在 ∧ 目标键取值符合预期），失败即用
  备份回滚。
- **幂等**：已达目标状态时**不写盘、不备份、不广播事件**。

### 验证

- `cargo test --lib` **168 项全通过**（其中技能相关 34 项，本版新增 29 项覆盖外科手术
  边界：CRLF 保留、块标量/纯多行标量后插入、嵌套映射同名键不误判、删键不多删空行、
  无末尾换行、幂等、可逆、非裸布尔与重复键拒绝、技能名官方字符集校验）。
- 新增 hermetic 集成测试 `skill_write_pipeline_test`（端到端 10 步：双根扫描 / rank 覆盖
  标注 / 停用写盘与备份字节一致 / 幂等 / 启用逐字节还原 / name 不符拒绝 / 根外路径拒绝 /
  非裸布尔拒绝 / 删除入回收站 / 回收站不被识别为技能 / 平铺 `.md` 技能可启停）。
- **真实数据回归**：对本机 `~/.agents/skills` 全部 **93 个真实技能**执行「停用 → 启用」
  往返，断言**逐字节回到原状** —— 结果 **93 个检查、0 个被拒绝、0 个字节差异**，
  其中 14 个已停用技能走幂等路径。CRLF 计数不变。
- `tsc --noEmit` / `tsc -p tsconfig.node.json --noEmit` / `vite build` /
  `cargo check --all-targets`（**零警告**）全部通过。

### 已知边界（诚实声明）

- 官方**不存在**任何脚本化手段可列举「当前生效的技能」：无技能 CLI、`--dump-config`
  不枚举技能也不暴露技能根、`skills/list` Remote 只读且仅 4 字段（无路径）。故脚本化
  证据止于「重新读取目标键」；**技能是否真的从模型可见目录消失需在 dsh GUI 中人工确认**
  （沿用 ADR-0006 D12 对 MCP 的同一处理方式）。
- **项目级技能不可管理**：项目根取决于 dsh **会话的工作区**（不是进程 CWD），启动器
  不可知；custom 根由 preset 声明且不出现于合成配置（官方刻意禁用 host 行，由各 preset
  挂载）。二者按 ADR-0007 D5 排除，面板不显示它们。
- `.trash` 无自动清理策略（面板显示残留计数）。

## [0.7.1] - 2026-09-10

> 本版为**弹窗宽度缺陷修复 + 前端设计/外观/排版全链路统一**。硬边界：未修改
> `deepseek-harness` 任何代码；未引入新依赖；纯前端（样式/布局/动效）与版本同步。

### 修复

- **弹窗宽度失效（P0）**：v0.7.0 的 MCP / 插件 / 技能 / 设置四个弹窗在 ≥640px 视口下
  **实际渲染宽度恒为 384px**，调用方声明的 `max-w-2xl` / `max-w-xl` / `max-w-lg` 全部是死代码。
  - **根因**：`src/components/ui/dialog.tsx` 的 `DialogContent` 基线类含 `sm:max-w-sm`；
    它与调用方的 `max-w-*` 特异性相同，而 Tailwind v4 把 `sm:` 媒体查询变体输出在
    **基础工具类之后**（构建产物中 `.sm\:max-w-sm` 位于 `.max-w-2xl` 之后），故恒定覆盖。
  - **证据**：真实浏览器实测四者 `getBoundingClientRect().width` 均为 384px、
    `computed max-width` 均为 `--container-sm`(24rem)，与声明的工具类不符。
  - **修复**：`DialogContent` 不再声明任何宽度（消除级联歧义），宽度交还调用方；
    四个管理面板统一为 **768px（48rem）＝修复前实测 384px 的两倍**，并以
    `w-[calc(100vw-2rem)]` 保证任意窗口尺寸下自动收缩、永不溢出。
- **原生控件外观脱离设计体系**：面板内的 `<select>` / `<textarea>` / `<input type=checkbox>`
  原为浏览器默认样式（方角、灰边、无焦点环），现统一为圆角 + 主题边框 + 焦点环
  （`.dsh-select` / `.dsh-textarea` / `.dsh-checkbox`），下拉展开项强制主题底色，
  消除深色主题下的白底闪烁。

### 新增

- **弹窗版式重构**：`DialogHeader` 改为 **sticky 常驻**（长列表滚动时标题与说明可见），
  新增 `DialogBody` 作为**唯一独立滚动区**，长内容不再把弹窗撑出视口。
- **统一动效令牌**：`--motion-fast/base/slow` + `--ease-standard/entrance` 集中定义，
  遮罩/弹窗/展开箭头/进度条/hover 全站共用一套时长与缓动；列表展开箭头增加 90° 旋转反馈，
  日志文件视图切换增加一次性淡入。
- **无障碍降级**：`prefers-reduced-motion: reduce` 下全部位移/缩放动效降为瞬时，
  仅保留 `animate-spin` 加载指示（避免被误判为卡死）。
- **容器查询自适应**：侧栏被拖窄至 <240px 时，底部四个管理入口自动收起为纯图标
  （`@container` + `.panel-entry-label`），不再挤压换行。

### 变更

- **全链路响应式加固**：四个管理面板与工具链/状态/版本面板的**工具条、表单行、列表行**
  全面 `flex-wrap` 化（窄宽度自动换行而非溢出），McpPanel 表单输入
  `flex-1 + min-w-*`、SettingsPanel 开关栅格 `grid-cols-1 sm:grid-cols-2`、
  SkillsPanel 下拉框 `w-full sm:w-40`。
- **交互状态细化**：面板列表行统一 `.dsh-list-row`（hover 提亮 + 描边过渡），
  SettingsPanel 开关行增加 hover 底色反馈；异步按钮补 `Loader2` 旋转态与
  「处理中…」文案（MCP 刷新/添加、插件同步/安装、技能应用/迁移、版本安装）。
- **图标语义统一**：弹窗标题栏图标与侧栏四个入口一一对应（Plug/Puzzle/Sparkles/Settings），
  每个弹窗补充一句功能说明（`DialogDescription`）。

## [0.7.0] - 2026-09-10

> 本版为 **ADR-0006（三类能力的官方合规收敛）** 施工结果，分两部分：**Part A** 新增 MCP Server
> 管理；**Part B** 对既有插件管理与技能共享做官方合规核验与修复。硬边界：未修改
> `deepseek-harness` 任何代码；未新增退出码；未引入新依赖。

### 新增（Part A — MCP Server 管理）

- **MCP Server 管理**：按 `serverName` 独立管理 `list / add / remove / enable / disable`
  （官方 `config.serverName`，工具命名空间 `mcp__<serverName>__<tool>`）。
  - 落点为机器级 `$DSH_HOME/cordis.patch.yml` 的**受管 MCP 区块**（marker `dsh-launcher mcp v1`），
    **两段式**：`- insert:` 声明段 + `- id:`/`disabled:` 定向段；块外内容逐字节保留。
  - `config` 采用**不透明保真**：以逐行原始文本存取，`!!js` 表达式、内嵌注释与未建模字段
    零损失；`enable` / `disable` / `remove` **绝不重渲染** `config`。
  - `list` 覆盖**合成树全量** mcp 行（含 bundle / profile patch / 用户手写 / `--patch`），
    每行标 `origin`（`managed` / `external`）与**来源层**（dump 段标签绝对路径）。
  - `remove` 语义二分：`managed` 真删除（声明 + 定向）；`external` **仅撤销定向覆盖**
    （声明仍在，服务器恢复默认启用），UI/CLI 文案明确区分。
  - 三类变更**都不重启 dsh**（受管 profile `web` 为 `patchReload: "live"`，官方就地热重载）。
- **底部四按钮入口**：`MCP | 插件 | 技能 | 设置`（MCP 最左）；新增 **MCP 面板**
  （与插件面板同构；`remove` 复用既有确认弹窗模式，文案区分 managed/external）。
- **危险字段护栏**：`failOnStartupError: true` 会使整个 harness 启动中止，故
  `list` 对该行打**危险徽章**并提示后果，结构化新增通道**不暴露**该字段（仅原始 YAML 通道可表达）。
- **CLI**：`dsh-launcher mcp list|add|remove|enable|disable [--json]`，`add` 支持全部官方字段
  选项（`--raw-config` 与结构化字段互斥），复用既有退出码分级（0/2/3/6/7/8）。
- **Tauri IPC**：`mcp_list` / `mcp_add` / `mcp_remove` / `mcp_set_state` + 事件 `mcp://changed`。

### 修复（Part B — 插件与技能的官方合规修复）

- **A1/A2（P0）技能共享真源改锚官方根（D17）**：真源由 `~/.agents/agent` 改为**官方
  `agentsHome` 根**（默认 `~/.agents`）——技能 = `~/.agents/skills`（官方 `skill-filesystem`
  的 `user-agents` 根，rank 500）；指令 = `~/.agents/AGENTS.md`。
  - **技能不再需要任何链接**（rank 500 原生覆盖）；`<dshHome>/CONTEXT.md` **不再建链接**
    （dsh 不读该文件）；仅 `<dshHome>/AGENTS.md` 保留一条指向 `~/.agents/AGENTS.md` 的链接。
  - 修复前现场：`~/.dsh` 三条链接**全部断链**、真源目录为空，而官方根有 93 个有效技能
    → 技能共享实际未生效、用户全局指令未被 dsh 读取。修复后 `~/.dsh` **无断链**、
    技能计数 **93**、指令内容与真源逐字节一致。
  - 新增 `dsh-launcher skill repair-links`：一次性、**可幂等重跑**的迁移动作 —— 修复指令链接、
    清理**启动器自己创建**的断链、保留一切真实文件/目录与用户自建链接（**零删除用户内容**）。
  - 资源状态新增 `native`（该资源不需要链接，真源已由官方扫描根覆盖）；判定链改为
    「链接 → 真实文件 → 原生根」，避免视图侧遗留断链被误判成 `native`。
- **A3/P6 失败回滚写入收敛 + 备份路径修正（D18）**：
  - `package.json` / `pnpm-lock.yaml` / `pnpm-workspace.yaml` / `cordis.patch.yml` 的写入
    收敛为**唯一一处** `rollback_after_failed_official_op()`，并显式登记为白名单**唯一例外**
    （官方无"回退到任意历史 lock 状态"的能力），其后**必跟**一次官方通道
    `dsh plugin … install` 收敛。
  - 备份落点由文件名改为**备份子路径**：修复了同目录下三个同名不同义文件互相覆盖、
    回滚会写入另一个文件备份内容的缺陷。
- **A4 删除 `allowBuilds` 规格（不补实现）**：该键属 **pnpm 配置**而非 dsh 官方接口，
  故删除"由启动器写入并复跑"的规格，改为把 pnpm 输出原样转发，由用户自行处理；
  面板文案同步更正（不再指示用户手改 pnpm 配置）。
- **A5 收窄验证口径**：明确 `--dump-config` **不启动插件**、只证明**配置合成层**的写入正确性，
  **不得**充当运行态生效证据；运行态可见证据只有 dsh stderr 的 logger 行 + 有界窗口
  （官方不存在列 MCP/插件状态或列工具的 CLI）。

### 变更

- **受管区块通用层**：`core/plugin/managed.rs` 抽出「按 marker 家族读写区块」的通用层
  （marker 前缀 / 版本 / 渲染器参数化），managed / shared / **mcp** 三个家族共用同一段
  定位、`[]` 占位符处理、块外逐字节保留与幂等判定。
- **同文件写入互斥**：新增按规范化路径区分的 `file_write_lock`（线程局部可重入），
  使 `$DSH_HOME/cordis.patch.yml` 上 ADR-0005 的 `shared` 区块与新 MCP 区块**写入互斥**。
- **构建脚本**：为 `tests/*` 产物嵌入 Common Controls v6 清单（`tauri build` 只为应用 bin 加），
  修复触及 tauri 依赖链的集成测试在加载期以 `STATUS_ENTRYPOINT_NOT_FOUND` 终止的问题。
- **文档**：`CONTEXT.md` 增 MCP 词条组并更新技能共享锚点（新增 `native` 资源状态、`needs_link`
  判定、`repair-links` 动作、白名单唯一例外）。
- **运行时文件系统**：新增备份目录 `%LOCALAPPDATA%\dsh-launcher\backups\mcp\<serverName>\<ts>\`；
  **未新增任何持久化状态文件**（MCP 的期望态与现状均由受管区块 + 合成树表达，磁盘即事实源）。

### 测试

- 单元测试 `139`（+47）、集成测试 `16`（`mcp_pipeline_test`，临时 `DSH_HOME` + 假 `dsh`）、
  Part B 合规回归 `15`（`part_b_compliance_test`）；ADR-0005 既有用例**零改动**全绿。
- 端到端脚本 `scripts/e2e-adr0006.ps1`（真实 dsh + 隔离 `DSH_HOME`）：**61 项断言全通过**。

## [0.6.0] - 2026-09-10

### 新增

- **插件管理（ADR-0005）**：按包名（ID）独立管理 `enabled / disabled / uninstalled` 生命周期。
  - 启停写 profile `cordis.patch.yml` 的**受管区块**（`- id:` + `disabled:`），由 dsh `live` 热重载，
    **无需重启**；装卸走官方 `dsh plugin --profile web add/remove` 通道，需要重启时自动
    stop→apply→start。
  - 明确状态机与非法转换拒绝（未安装/普通依赖/表达式控制的行都拒绝启停，退出码 2）。
  - 幂等：期望态与磁盘一致时返回 `unchanged`，不落盘、不重启、不发事件。
  - **upstream 插件自动同步**：npm 源比对 registry 最新版本，git 源 `git ls-remote` 比对并
    钉 commit；后台启动同步（设置项 `autoSyncPlugins`，默认开）+ 面板/CLI 手动同步。
  - **自研插件隔离**：`link:`/`file:`/相对或绝对路径/tarball 判定为 `in-house`，同步任务
    永不改动。
  - 启动崩溃自动归因：从 dsh stderr 命中插件则**禁用其行**（可逆），不再硬编码卸载 dshmarket。
- **技能共享管理（ADR-0005）**：以 `~/.agents/agent` 为唯一真源，统一 `skills/`、`AGENTS.md`、
  `CONTEXT.md`。**（注：0.7.0 的 ADR-0006 D17 已把真源改锚到官方 `agentsHome` 根 ——
  技能 = `~/.agents/skills`、指令 = `~/.agents/AGENTS.md`；本条保留原发布时的事实。）**
  - Mode L（默认）：`~/.dsh` 下同名资源建立链接（目录用 junction 免特权，文件用符号链接）；
  - Mode C（降级）：无文件符号链接权限时，写 `$DSH_HOME/cordis.patch.yml` 的 shared 区块
    （`skill-filesystem.agentsHome` / `agent-instructions.dshHome`），dsh 直接读真源；
  - 冲突资源（真实文件/目录）**绝不自动删除**，只提供预演与显式迁移（原文件改名保留）。
- **底部入口按钮组**：侧栏底部由单个"设置"按钮改为 `插件 | 技能 | 设置` 三按钮组（顺序固定），
  分别打开对应 Dialog。
- **无 GUI CLI**：`dsh-launcher plugin|skill ...`（与 GUI 共用同一 core，供脚本化验收），
  支持 `--json` 与分级退出码。

### 变更

- 配置新增 `autoSyncPlugins`（滑动开关第 7 项）。
- 新增状态文件：`%APPDATA%\dsh-launcher\plugins.json`、`skills.json`；
  新增备份目录 `%LOCALAPPDATA%\dsh-launcher\backups\plugins\`。

## [0.5.7] - 2026-09-04

### 修复

- **"启动 dsh 后自动开内嵌窗口"失败（弹窗卡"正在打开"直到超时重试），手动点"内嵌
  打开"却正常**：
  - 根因：冷启动时序下 URL 获取与 HTTP 探测**分离且 url 固定**——旧实现先
    `waitForWebUrl` 一次性拿到 url（可能是旧/过期 token，dsh 重启后 token 变化，
    启动瞬间内存/缓存可能残留上一轮值），再用**同一 url** 探测 30s。实测 dsh 对
    **无效 token 返回 400 Bad Request**（非 2xx/3xx）→ 探测死循环超时；而手动打开
    时 dsh 稳定、url 为当前有效 token（返回 303）→ 秒过。差异即"自动失败、手动成功"。
  - 修复：前端 `openWithGuide` 与 Rust `open_web_gui_window` 均改为 **URL 获取与
    HTTP 探测一体化循环**——每次迭代取**最新** getWebUrl（tail 捕获当前 dsh 新
    token，token 变化自然拿到新值；Rust 侧不再读旧缓存兜底），仅含 token 的 URL
    参与 `web_ready`（2xx/3xx 判定）探测，通过即开窗，不通过继续取最新 URL 重试
    （≤40s）。有效 token + 路由就绪（303）必然通过，冷启动自动开窗不再卡死。
  - 移除不再使用的 `waitForWebUrl`（内联入一体化循环）。

## [0.5.6] - 2026-09-04

### 修复

- **开窗探测误判导致弹窗卡"正在打开"30s 超时**：
  - 根因：`web_ready` 判定 HTTP **200** 才就绪，但实测 dsh web 对带 token 请求返回
    **303 See Other**（重定向到 `/` 走会话交换，浏览器跟随后才 200）——裸 HTTP GET
    永远到不了 200 → 探测永不通过 → 前端轮询 30s 超时报"HTTP 未返回 200"。
  - 修复：就绪判据改为 **2xx/3xx**（200/301/302/303/307 均视为 HTTP server 完整响应、
    路由已挂）；404（冷启动路由未挂）/401/5xx/连接失败仍视为未就绪。新增
    `http_status_is_success` 纯函数 + 单测覆盖。
  - 效果：内嵌与外部浏览器两条路径都能在 dsh 真正可服务后立即打开（不再 30s 卡死）。
- **dsh web 访问地址（token URL）明文写入日志与前端日志流**：
  - 产品决策（所有者明确要求）：用户需从日志复制**完整带 token 的访问地址**在外部
    浏览器手动打开（裸 URL 会被 dsh 401 "authentication required" 拒绝）。
  - 安全权衡说明：日志仅本机用户可读写（LOCALAPPDATA），不再对 token 打码；
    dsh stdout 落盘文件（dsh-web-stdout.log）本就不打码，此改动使统一日志
    （launcher 侧）与前端日志面板同样可见完整 token。
- 外部浏览器打开按钮：随就绪探测修复，带 token URL 正常打开（此前同样被探测卡住）。

## [0.5.5] - 2026-09-04

### 修复

- **冷启动弹出内嵌窗口命中 404（"找不到此 127.0.0.1 页 / HTTP ERROR 404"），须关闭重开**：
  - 根因：端口监听/TCP 通 ≠ HTTP 路由就绪。冷启动时 dsh 先监听端口、后挂 SPA 路由，
    期间带 token 请求返回 **404**；此前"等 token + 固定 1s"是启发式，token 打印瞬间
    HTTP 路由可能仍未挂好 → WebView2 首载命中 404 错误页且不自动恢复。
  - 修复：改为**真实 HTTP 就绪探测**——新增 Rust `core::port::web_ready()`（TcpStream
    手写最小 HTTP/1.1 GET，请求带 token 完整 URL，读状态行判定 **200**）+ IPC 命令
    `probe_web_ready`。
    - 前端 `openWithGuide`：拿到 token URL 后轮询 `probeWebReady` 直到 HTTP 200
      （≤30s，700ms 间隔）才开窗；
    - Rust `open_web_gui_window`（快捷方式/autoOpen 路径）：同步轮询 `web_ready`
      （≤20s）再建窗。
  - 效果：内嵌窗口只在 dsh web **真正可服务（HTTP 200）** 后弹出，冷启动首载即成功，
    不再出现 404 需手动重开。

## [0.5.4] - 2026-09-04

### 修复

- **启动后内嵌窗口在 dsh 打印新 token 之前过早弹出**（弹窗显示"打开内嵌窗口…"后立即出窗，
  需手动重开）：
  - 根因：`process.rs start_locked` 启动新 dsh 时**不清除上一轮的 token 缓存**
    （last-web-url 文件）。冷启动/异常退出后重启，旧缓存残留 → 前端 `waitForWebUrl`
    经 `get_web_url` 兜底读到**旧 token** 秒回 → 不等当前 dsh 输出新 token
    （`dsh web: http://127.0.0.1:3080/?token=…`）窗口就弹出 → 旧 token 无效 → 首载失败。
  - 修复：
    1. `start_locked` spawn 成功后**清空内存 web_url + last-web-url 缓存**，前端只能
       轮询到当前进程输出的新 token 才放行；
    2. 前端 `waitForWebUrl` 增加校验：仅接受**含 `token=` 的完整 URL** 才返回
       （裸 URL / 无 token 值一律继续等，杜绝 401 死窗口）。

## [0.5.3] - 2026-09-04

### 变更

- **启动后打开内嵌窗口增加 1 秒就绪缓冲**：
  - 前端（StatusCard openWithGuide）与 Rust（open_web_gui_window）两条路径统一：
    拿到带 token 完整 URL（dsh 打印 `dsh web: http://127.0.0.1:<port>/?token=…`）后
    **再等 1 秒**才创建内嵌窗口。
  - 原因：token 刚输出的瞬间 dsh web 服务/前端资源可能仍在预热（首屏资源、插件
    serve 未完），立刻开窗会首载失败/白屏，需手动重开一遍；1 秒缓冲后首载即成功。
  - 前端 waitForWebUrl 已保证等 token 出现（≤40s），Rust 路径等 token ≤10s，两者
    之后统一 +1 秒缓冲，与"完全启动后 1 秒弹出"的用户预期一致。

## [0.5.2] - 2026-09-04

### 修复

- **内嵌 Web GUI 白屏 + 所有窗口无法关闭（需任务管理器强杀）——主线程死锁**：
  - 根因：`create_web_gui_window` 命令在 async 运行时线程执行同步 fn，内部
    `run_on_main_thread(建窗)` + `std::sync::mpsc::recv()` **阻塞等待主线程建窗结果**。
    当主线程正被 WebView2 的同步窗口消息占用（内嵌 dsh UI 页面加载/交互时常见，
    Windows 窗口消息模型下主线程处理 WebView 消息期间不会让出给 run_on_main_thread
    排队的闭包）→ 主线程与等待它的命令线程**交叉死锁** → 内嵌窗口停在白屏、主窗口
    及所有窗口事件循环卡死、无法关闭。
  - 修复：命令改为 **async fn，把建窗调度到主线程后立即返回（fire-and-forget），
    不跨线程阻塞等待**；窗口创建仍在主线程完成（Tauri 要求），创建失败由内部日志
    记录。前端不再依赖返回的窗口 label（图标在创建时已同步设置）。
- 移除 create_web_gui_window 返回 label 的契约（前端 lib/tauri.ts `createWebGuiWindow`
  签名不变，仅不再消费返回值）。
- **内嵌窗口偶发创建失败（"the underlying handle is not available"）**：`apply_window_icon`
  在 build() 后立即取 hwnd 的竞态失败**不再阻断窗口创建**（恢复 0.4.14 语义：失败仅记警告，
  窗口照常打开）。此前 `?` 传播会把已建成的窗口整体判失败 → 不开窗。
- **日志全域化（不允许丢失任何日志）**：
  - 新增全局 **panic hook**：任何线程 panic 落盘 `logs/crash.log` + 经 Logger 记录
    （release 无控制台时 panic 此前完全不可见 → 静默崩溃无痕）。
  - 关键路径 `eprintln!`（主窗口图标失败、外链打开失败、Tauri 启动失败）改经 Logger
    落盘，release 下不再丢失。
  - dsh stdout/stderr 已实时落盘 `dsh-web-stdout/stderr.log` 并逐行进日志流；崩溃
    尾部不完整行有 flush 兜底，保证 dsh 侧日志不丢。

## [0.5.1] - 2026-09-04

### 修复

- **内嵌 Web GUI 窗口白屏**：
  - `on_new_window` 不再一律 Deny+系统浏览器——回环源（127.0.0.1/localhost/::1，
    dsh Web UI 自身）的新窗口请求 **Allow 进程内放行**，仅非回环 http(s) 外链交
    系统浏览器并 Deny。此前 0.5.0 把前端"内嵌打开"统一到 Rust builder（带
    on_new_window）后，dsh UI 内部以 window.open 形式打开的界面被全部掐掉 →
    白屏（0.4.14 前端 new WebviewWindow 路径无此回调故正常）。
- **启动按钮状态反馈不即时**：
  - 前端点"启动"立即乐观置 `starting`（按钮转"启动中…"+ 转圈），不再等 5s 轮询；
  - Rust `start_dsh` 启动后**等待端口就绪（≤15s）再返回**，前端 await 完成即置
    `running`，状态徽标即时变"运行中"（此前 spawn 即返回，要等轮询才收敛）。

### 变更

- **点"启动"成功后自动打开内嵌 Web GUI**（产品优化）：启动就绪后自动走
  openWithGuide(embedded) → 等带 token URL → Rust 统一创建路径开窗（图标清晰），
  无需再手动点"内嵌打开"。

## [0.5.0] - 2026-09-04

### 修复

- **修复内嵌 WebView2 子窗口任务栏图标模糊（主窗口清晰、内嵌窗口模糊）**：
  - 根因（源码级证据链）：tao `set_window_icon` 只发 WM_SETICON ICON_SMALL；tauri
    Windows 默认窗口图标取 `icons/icon.ico` **第 0 帧（16×16）** 注入；前端"内嵌打开"
    （StatusCard `new WebviewWindow`）创建窗口**不带 icon** → 任务栏按钮首帧以默认
    exe 16px 图标绘制，`tauri://created` 后置补发 `set_web_gui_icon`（512 SMALL +
    256 BIG）**晚于按钮首帧** → 模糊/闪烁。而 Rust 路径（桌面快捷方式/自动打开）在
    builder 预置 512px 图标 + 创建即补发，故主窗口与 Rust 入口窗口清晰。
  - 修复：**窗口创建统一收敛到 Rust 单一路径**——新增 `create_web_gui_window` IPC
    命令复用 `WebviewWindowBuilder`（预置 512px 图标 + 创建后同步
    `apply_window_icon` SMALL+ICON_BIG 256px 原生 HICON）；前端"内嵌打开"改调该命令，
    不再 `new WebviewWindow`。三条入口（内嵌按钮/桌面快捷方式/自动打开）行为完全一致，
    任务栏图标源与设置时机对齐主窗口；失败经 Logger 可查（不再仅 stderr）。
- **用户 PATH 变量引用固化修复（高危环境破坏）**：`user_path()` 改用
  `RRF_NOEXPAND` 读取注册表原始值，prepend/remove 不再把 `%SystemRoot%`/
  `%JAVA_HOME%` 等展开成绝对路径写回（此前会永久固化变量引用、可能超长）。
- **GitHub Token 加密失败不再静默明文落盘**：`config.rs::save` 在 DPAPI
  CryptProtectData 失败时返回 Err（此前 `unwrap_or_else` 静默降级明文写盘）。
- **日志 10MB 切割静默失效修复**：写入句柄显式共享删除标志（FILE_SHARE_DELETE），
  避免与 tail 只读句柄共存时 rotate rename 返回 ERROR_SHARING_VIOLATION。
- **tasklist 存活探测误判修复**：`process_alive` 改为按行解析第 2 列 PID 精确匹配
  （此前 `contains(pid字符串)` 会把内存/时间列含同数字子串的任务误判为存活），
  新增单元测试覆盖。
- **`run_with_timeout`/流式命令强杀等待死循环护栏**：taskkill 失败且进程不退时
  最多等 3s 返回错误，不再无限 `try_wait` 卡死调用线程。
- **重启前保存端口**：StatusCard 重启前先 savePort（此前 UI 显示新端口、Rust 按
  旧端口重启，误导）。
- **日志补流性能与去重**：LogPanel 补流只取 `yyyy-MM-dd.log`（排除 dsh-web-*.log
  进程落盘文件），且只保留尾部 2000 行（此前可能全量解析 10MB+ 并大量重复）。
- **死代码清理**：SettingsPanel 非 embedded 外壳分支（无调用方）、冗余 import、
  pathutil 废弃 expand_env 等。

### 变更

- 内嵌窗口 label 追加进程内计数+随机混合后缀（防极端同毫秒撞 label）。
- 新增 IPC `create_web_gui_window`（前端 lib/tauri.ts 封装 `createWebGuiWindow`）。

## [0.4.14] - 2026-09-04

### 修复

- **修复内嵌 Web GUI（deepseek-harness Web UI）无法打开会话中的超链接**：
  - 根因（三层）：
    1. dsh Web UI 将 markdown 中的 http/https 外链渲染为 `target="_blank"` 锚点（dsh 侧源码行为，本启动器不改 dsh）。
    2. launcher 注册的 opener 插件（`tauri_plugin_opener::init()`，默认 `open_js_links_on_click: true`）向**每个** WebView 注入点击拦截脚本：左键点击 `_blank` 外链会 `preventDefault()` 并改走 `plugin:opener|open_url` IPC。
    3. 该 IPC 受 Tauri 2 ACL 管控：原 `capabilities/default.json` 仅授予 `main` 窗口 `opener:default`，内嵌 `dsh-web-gui-*` 窗口**没有任何 capability** → `open_url` 被静默拒绝（release 下 "not allowed by ACL"），点击无任何反应。
  - 修复：
    - 新增 `src-tauri/capabilities/dsh-web-gui.json`：对 `dsh-web-gui-*` 窗口放行 `opener:default`，`remote.urls` 覆盖 `http(s)://127.0.0.1:*` 与 `http(s)://localhost:*`（dsh Web GUI 的 loopback 服务地址；缺省组件自动补全配符，见 tauri URLPattern 语义）。除 opener 外**不授予任何本地文件/系统命令权限**。
    - `lib.rs` 内嵌窗口 builder 增加 Rust 兜底：`.on_navigation(|_| true)` 显式放行一切导航（窗口只作 dsh Web UI 载体）；`.on_new_window()` 收到 WebView2 原生新窗口请求（`window.open` 等）时用系统默认浏览器打开目标 URL 并 `Deny`，杜绝进程内游离 WebView2 子窗口。
  - 效果：点击会话内 http/https 超链接 → 系统默认浏览器（Edge/Chrome 等）打开新标签；ctrl/shift 点击与 `window.open` 类请求一致处理。
  - 验证：tsc / vite build / cargo check --all-targets（0 警告）。

## [0.4.13] - 2026-09-04

### 变更

- **全链路审计修复（依据 AUDIT_REPORT §2–§5）**：
  - **进程安全**：收养/停止/端口清剿前校验监听进程确为 dsh（命令行特征），不再强杀占用同端口的无关进程。
  - **假功能修复**：npm registry 镜像真正生效（npm view / npm install -g / pnpm install 注入 --registry）；GitHub tag 列表改 semver 数值排序（0.9<0.10、rc.9<rc.10）；工具链检测产出 `mismatch`（Node 22.19+/24+、Git 2.26+、Python 3.10+ 门槛比较）。
  - **凭据安全**：GitHub Token 落盘 DPAPI 加密（Windows）、不再回传前端明文、git 认证改环境变量注入（不进命令行）；dsh web token 的日志口径见 v0.5.6（**明文**，产品决策；本条早期表述"日志打码"已被 v0.5.6 取代，ADR-0009 D5 于 0.9.0 复核确认保留明文，并删除已成死代码的 `redact_web_token`）。
  - **可靠性**：子进程/网络操作全部加超时与强杀（查询/下载/安装/UAC 分档 60s~20min，流式命令 30min 看门狗）；配置读写加互斥与原子写；Logger/托盘构建失败降级不再 panic；新增后端每 5s 状态对账（收养实例退出收敛/外部启动自动接管）；monitor 收尾按 pid 归属复位，防覆盖重启新实例。
  - **前端**：面板宽度与对侧开关联动重算（reclamp 接线）；日志流增量格式化消除 O(n²)；贴顶滚动不再打断阅读；进度清理定时器登记、打开流程取消令牌下渗；版本平局比较按 semver 数值化（rc.9<rc.10）；开关/token 表单陈旧闭包与明文回显修正。
  - **构建/CI/发布**：`tsc -p tsconfig.node.json --noEmit` 纳入（vite.config.ts 首获类型检查，补 @types/node）；cargo check --all-targets；补离线集成测试，网络测试 `#[ignore]`；release 流程 bump 后显式打 tag、决策前同步远端 master、skip-release 不再自动递增；构建期依赖归入 devDependencies；清理死代码/重复实现与注释/文档漂移（AGENTS.md 悬空引用等）。
  - 验证：tsc / vite build / cargo check --all-targets（0 警告）/ cargo test --lib 35 通过 / 6 个离线集成测试通过。

## [0.4.12] - 2026-09-03

### 变更

- **移除版本管理面板通道按钮上的版本数角标**：
  - 原按钮组（GitHub 通道 / npm 通道）右侧显示通道可用版本总数角标（如 GitHub 10、
    npm 15）。该数字为远端拉取到的全部版本数，而列表只展示最新 8 个 → 与所见不一致
    易误导。移除角标，按钮仅保留通道名称，干净无歧义。
  - 版本 0.4.12。

## [0.4.11] - 2026-09-03

### 变更

- **优化内嵌打开逻辑：自动拉起 dsh 并等待就绪后才开窗 + 进度小弹窗**：
  - 此前点击“内嵌打开/外部浏览器”时若 dsh 未运行，直接 toast 报错要求手动先点“启动”
    ——流程割裂；若 dsh 冷启动中则静默轮询无感知。
  - 现在（components/StatusCard.tsx）：点击后弹出**进度引导弹窗**，阶段化展示
    （检查状态 → 启动 dsh → 等待就绪(端口监听) → 获取访问地址 → 打开窗口），
    每阶段推进度条与文案；dsh 未运行/出错时**自动调用 startDsh 拉起**（含端口校验、
    端口占用由 Rust 端报错），停止中则等待停止完成后自动重启；就绪后拿到带 token 的
    完整 URL 才创建内嵌窗口/打开外部浏览器（杜绝 401/拒绝连接死窗口）。
  - 弹窗含取消、失败重试按钮（重试走 force 重开流程）；用流程序号（useRef）防竞态——
    取消/重开后旧流程异步回调不再污染状态；未安装 dsh 时弹窗明确提示先安装。
  - 验证：tsc / vite / cargo release / NSIS 构建通过。版本 0.4.11。

## [0.4.10] - 2026-09-03

### 修复

- **卸载 Git / Python 失败：“Start-Process : 系统找不到指定的文件”**（真实客户机
  192.168.3.122 实测复现 + 验证）：
  - 现象：点卸载 Git / Python → 日志报 `Start-Process -FilePath 'C:\Program'`
    `-ArgumentList 'Files\Git\unins00...'`（路径在**第一个空格处截断**）→
    “系统找不到指定的文件”。
  - 根因：注册表 UninstallString 是**整体引号包裹**的路径，如
    `"C:\Program Files\Git\unins000.exe"`、
    `"C:\Users\...\Package Cache\{...}\python-3.14.0-amd64.exe"  /uninstall`。
    core/toolchain.rs 的 find_uninstall_entry 在收集候选时执行
    `trim_matches('"')` **提前剥掉首尾引号** → 返回值变成无引号含空格的裸路径
    → split_uninstall_cmd 按“无引号：第一个空白前为 exe”切分 → exe 截断为
    `C:\Program` / `C:\Users\Administrator\AppData\Local\Package` →
    Start-Process 找不到该“文件”。
  - 修复：find_uninstall_entry **保留原始 UninstallString（含引号）**，引号解析统一
    交给已正确支持引号路径的 split_uninstall_cmd（对 Git 整体引号、Python 引号+
    参数两种真实格式均正确解析）。
  - 验证：测试机 192.168.3.122 实测——旧逻辑 exe=C:\Program（不存在）复现报错；
    修复后 exe=C:\Program Files\Git\unins000.exe 与完整 Python 缓存路径均解析正确
    且文件存在；新增 test_split_uninstall_cmd_real_registry_values 回归测试
    （真实注册表值 Git/Python）。版本 0.4.10。

## [0.4.9] - 2026-09-03

### 变更

- **版本管理面板：双通道列表融合 + 按钮组切换**：
  - npm / GitHub 两个版本列表不再左右双列展示，改为顶部按钮组（GitHub 通道 / npm 通道）
    切换单一列表，**默认 GitHub**；切换只切展示（两列表进入时已并发预加载）→ 零网络
    开销、零状态丢失，来回切换无 bug。
  - 原每个通道独立的刷新按钮合并为**单一“刷新列表”按钮**（并发刷新两通道），
    列表项显示通道 badge + 版本数角标。
- **安装任意通道版本自动卸载对侧通道（全局单版本彻底化）**：
  - 此前 ADR-0003（全局单版本）仅约定语义，实际两通道各自安装：npm 全局包残留
    node_modules、GitHub 源码目录残留数 GB，dsh.cmd 同名互相覆盖，切换不干净。
  - 现在（commands/version.rs）：安装 npm 通道前自动清理 GitHub 通道（删源码目录 +
    自家 shim）；安装 GitHub 通道前自动卸载 npm 全局包（npm uninstall -g）。
    切换前统一优雅停止 dsh（ADR-0002）。DSH_HOME 用户数据不受影响。
  - uninstall 命令重构复用同一套清理函数（消除重复逻辑）。
  - 前端：跨通道安装后自动切换到安装通道 Tab；按钮文案区分“切换安装”。
  - 版本 0.4.9。

## [0.4.8] - 2026-09-03

### 修复

- **工具链（Python/Node/Git）安装时进度条无法实时跟踪下载进度**：
  - 现象：点击安装 Python/Node/Git，进度条停在 0% 不动，直到几十 MB 下载完才跳到
    100%，安装阶段也无任何反馈 → 用户感知“卡死/无响应”。
  - 根因：core/toolchain.rs 的 download() 用 PowerShell Invoke-WebRequest **一次性同步
    等待**（Command::output() 阻塞至下载结束），期间不推任何 install://progress 事件；
    Python 安装阶段 Start-Process -Wait 同样静默等待安装器退出。
  - 修复：
    - 新增 download_with_progress()：先 Invoke-WebRequest -Method Head 拿
      Content-Length（总字节），后台线程每 200ms 轮询目标文件已下载字节，按占比推
      progress(Download, pct, "下载中 x.xMB / y.yMB (pct%)")；拿不到总长度时退化
      为“已下载 x.x MB”不定量消息。下载结束统一推 100。
    - install_python()：安装阶段（Start-Process -Wait 期间）后台线程每 1s 轮询注册表
      PythonCore 是否已写入：已写入推 90%（收尾中），否则每 3s 推“已等待 N 秒（请在
      UAC 弹窗确认）”——安装全程有实时反馈，不再静默卡住。
    - Node/Git 下载统一改走 download_with_progress（三处调用全部迁移）；
    - 新增 download_percent 纯函数 + 单测（封顶 99，100 由完成路径推）。
  - 验证：真实下载 27.4MB Python 包，200ms 轮询捕获 11 个中间进度点（1%→99%）；
    HEAD Content-Length 解析正确；26 个 lib 单测 + 4 个离线集成测试全过；
    tsc / cargo release 构建通过。版本 0.4.8。

## [0.4.7] - 2026-09-03

### 修复

- **CI 工作流失败（cargo test --lib 在英文 runner 挂 3 个 GBK 单测）**：
  - 现象：push 触发 CI 的"单元测试（cargo test --lib）"步骤失败，Release 工作流（不跑 test）不受影响
    （v0.4.5/v0.4.6 的 CI 均因此失败）。
  - 根因：src/core/text.rs 的 test_gbk_cmd_error / test_gbk_with_ascii_prefix 与
    src/core/process.rs 的 test_decode_console_text_gbk 硬编码"GBK 字节 → 应解码出中文"断言。
    本机（中文 Windows，ACP=936）通过；GitHub Actions windows-latest runner 是英文系统
    （ACP=1252），decode() 按 GetACP 动态选择代码页（设计如此），GBK 字节被按 CP1252 解码
    成乱码 → 中文断言失败（CI 日志可见 `´íÎó: ÎÞ·¨ÖÕÖ¹...` 乱码）。
  - 修复：测试自适应当前系统代码页——中文（936）环境断言完整中文还原；
    非中文环境（英文 CI）断言 ASCII 片段保留（GBK 中文外的 ASCII 字节 1:1 保留，
    如 "PID 3108" / "npm"），并验证不 panic。新增 text::is_cjk_system_for_test 辅助。
  - 验证：本地（中文 936）25 个 lib 测试 + 4 个集成测试全过；CP1252 模拟验证 ASCII 断言成立。
  - 版本 0.4.7。

## [0.4.6] - 2026-09-03

### 修复

- **致命 bug：启动 dsh-launcher 后 node.exe 进程指数爆炸、停不下来**（用户机实测）：
  - 现象：启动器启动/状态刷新后，node.exe（corepack pnpm.js dsh）与 cmd.exe 数量
    指数级增长（10 秒内数百个），任务管理器里“频繁重复打开 node.exe”；WebView2
    进程（msedgewebview2）也随之堆积（启动器每次重试/卡顿都新建内嵌窗口进程组）。
  - 根因：卸载 dsh（GitHub 通道）后残留了指向已清空安装目录的全局 shim
    `dsh.cmd`（内容：`cd /d <github-dsh 安装目录> && pnpm dsh %*`）。启动器每次
    探测 dsh（`dsh --version`）都执行该 shim → pnpm 在空目录找不到本地 dsh 定义，
    把 `dsh` 当外部命令解析 → 又命中 PATH 中同一 dsh.cmd → 递归 spawn
    `cmd → node(pnpm) → cmd → ...`，进程树指数爆炸（实测进程链深达 10+ 层）。
  - 修复（core/github.rs + commands/version.rs + core/process.rs）：
    - 新增 `DshProbe` 枚举 + `probe_dsh_command()`：**纯静态解析** PATH 中 dsh.cmd
      内容与目标目录存在性（零进程开销），判定 OwnedShimOk / OwnedShimBroken /
      Foreign / None；本启动器 shim 指向缺失/空目录时判定 OwnedShimBroken；
    - `get_installed_version()`（前端挂载/轮询频繁调用）与 `start_locked()`
      （点“启动”）在**执行任何 dsh 命令前**先 probe：OwnedShimBroken 直接短路
      （视为未安装 / 返回明确错误），绝不再执行会递归爆炸的 `pnpm dsh`；
    - 新单元测试覆盖 shim 内容识别与 cd 目标目录提取（含 npm 全局 shim 不误判）。
  - 版本 0.4.6。

## [0.4.5] - 2026-09-03

### 变更

- fix: 分发到其他 Windows 客户机的 Web GUI 打不开（401 认证页 / localhost 拒绝连接）
- 分发机器实测根因：
- 启动器（Tauri GUI 进程 + CREATE_NO_WINDOW）把 dsh web stdout 接成匿名管道时，
- 在部分客户机上输出不实时到达（token URL 直到 dsh 被杀才出现），启动器永远
- 捕获不到 token → Web GUI 回退裸 URL → 401 "dsh web authentication required"；
- 开窗时机早于 dsh 端口就绪 → 窗口直接 ERR_CONNECTION_REFUSED。
- 修复：
- core/process.rs：dsh 子进程 stdout/stderr 由管道改为重定向落盘文件
- （logs/dsh-web-stdout.log、dsh-web-stderr.log，每次启动截断），监视线程轮询
- tail 文件实时喂日志并捕获 token URL —— 摆脱 GUI 父进程管道缓冲的不确定性；
- lib.rs：open_web_gui_window（桌面快捷方式/自动打开）后台线程先等端口监听
- （≤8s）再等带 token 完整 URL（≤10s），拿到才开窗；dsh 未运行/无 URL 时聚焦
- 主窗口并落日志提示，不再打开 401/拒绝连接死窗口（窗口创建仍回主线程）；
- components/StatusCard.tsx：内嵌打开/外部浏览器改为等待 token URL（最长 40s），
- 不再 10s 后回退裸 URL（401 入口）；dsh 未运行直接提示先启动，等待期间按钮禁用；
- core/github.rs：git 解析加入 Git for Windows 常见安装目录绝对路径兜底
- （工具链安装后启动器 PATH 快照不刷新导致 "git ls-remote: program not found"）。
- 验证：全新 Windows 客户机端到端通过 —— 自动启动 dsh、token URL 3s 内实时
- 进入日志、用捕获链接鉴权握手 200（裸 URL 401），内嵌 Web GUI 正常打开；
- GitHub 通道版本列表正常。版本 0.4.5。

## [0.4.3] - 2026-09-03

### 修复

- **分发到其他 Windows 客户机后 dsh 服务启动失败**（dsh CLI 报
  “--profile <name> is required”、退出码 1，真实客户机 192.168.3.124 实测）：
  - 根因 1（npm 全局分支丢参数）：v0.4.2 重构时，“PATH 中 dsh 为 npm 全局包，直接用其启动”
    分支改为直接启动 dsh，却丢失了 “web --port <p> --no-open” 参数 → dsh 裸跑不带 profile
    → CLI 报 “--profile <name> is required” 后退出 → 启动器探测到端口未监听，误入
    “自动修复不兼容插件”流程（实际与插件无关）；
  - 根因 2（本启动器 shim 被误判为 npm 包）：shim 归属探测用不注入 PATH 的 where 探测；
    分发机器上 npm 全局 prefix 被固定为启动器管理的 node_dir（LOCALAPPDATA 下 dsh-launcher
    的 toolchain node 目录，不在启动器进程的 PATH 快照里）→ 启动器刚创建的 GitHub 通道
    dsh.cmd shim 探测不到 → 被误判成“npm 全局包” → 再次踩根因 1；
  - 修复：① npm 全局真 dsh 分支补全 “web --port <p> --no-open” 参数；
    ② shim 探测改用与真实 dsh spawn 相同的 PATH（注入 node_dir），分发机器也能识别
    本启动器 shim；③ 启动决策重构：GitHub 安装目录存在（apps/cli/src/bin.ts）时一律
    直接 node 启动 bin.ts（本启动器 shim / PATH 无 dsh 也走此路，进程浅、token 实时、
    taskkill 干净）；仅 PATH 中 dsh.cmd 确为 npm 全局包时才走 cmd 包装的 dsh web；
    shim 指向的安装目录缺失时报明确错误（不再误启）。

## [0.4.2] - 2026-09-03

### 修复

- **内嵌 Web GUI 报 “dsh web authentication required; reopen the URL printed by dsh web”**（
  真实客户机 192.168.3.124 实测）：
  - 根因 1（token 捕获延迟）：GitHub 通道 dsh 经 dsh.cmd shim（`cd 安装目录 && pnpm dsh`）
    启动，Windows 下 pnpm 多层 cmd/node 嵌套（实测 5 层），dsh web 的 token stdout
    在多级管道缓冲下**不实时到达**启动器 → web_url 捕获不到新 token → 内嵌窗口用裸 URL
    或旧 token 访问 → 401；
  - 根因 2（残留僵尸占端口）：进程树 5 层导致 `taskkill /T` 杀不净，停止后残留 dsh web
    node 孤儿进程继续占 3080（持旧 token）→ 新旧 token 错乱叠加 401。
  - 修复：GitHub 通道启动改为**直接 node 执行** `node --import tsx/esm
    apps/cli/src/bin.ts web --port <p> --no-open`（cwd=安装目录，node.exe 绝对路径），
    进程树浅、stdout 实时输出 token URL、taskkill 可干净终止；
  - 修复：停止后若端口仍监听（taskkill 树杀不净），按端口查实际监听 PID 逐个强杀直到
    端口释放（最多 5 轮兑底清剿）；
  - 前端 resolveWebUrl 轮询从 3 秒延长至 10 秒（dsh 冷启动 + token 打印时间），
    并依赖 Rust 侧内存捕获 → 日志兜底双链路。

## [0.4.1] - 2026-09-03

### 修复

- **Python 安装后仍显示缺失（状态检测不准确）**：
  - 根因：python.org 官方安装器（per-user /PrependPath）把 python.exe 写入用户 PATH
    （HKCU），启动器进程环境**快照不刷新** → `python --version` 探测 miss → 面板显示缺失；
  - `detect_python` 增加多级兑底：`python` → `python3` → `py -3` → **注册表
    PythonCore\<ver>\InstallPath 内的 python.exe**（注册表实时可见，无快照问题）；
  - 新增 `core/pathutil::python_install_dirs`：枚举 HKCU+HKLM PythonCore 各版本安装目录
    （python.exe 存在才收，去重，版本降序）+ 常见用户级 Programs\Python 兑底；
  - `install_python` 安装器实为 `Start-Process -Wait` 同步完成，返回时安装已结束：
    改为完成后读注册表确认并报“已安装完成”，不再提示“等待 UAC 后手动刷新”；
    安装成功（含 Python）后 Rust 广播 `toolchain://changed` → 前端自动刷新为已就绪；
  - 前端提示语义修正：仅 **Git 安装** 为真异步 UAC（无 -Wait）需提示等待，
    Python 安装/卸载、Git 卸载均为同步完成，直接显示成功（不再误提示需手动重新检测）。
- **Git 检测兑底**：PATH 无 git 时回退常见安装目录 `C:\Program Files\Git\cmd\git.exe`
  （用户安装 Git 未勾选“加入 PATH”时也能识别）。

## [0.4.0] - 2026-09-03

### 新增

- **工具链管理功能**（ToolchainPanel 全面升级）：
  - 每个工具链条目支持**卸载**：Node 删除用户级安装目录并清理用户 PATH 条目；
    Git/Python 从注册表卸载入口（UninstallString）定位官方卸载器并提权启动（UAC 确认）；
  - **批量一键安装**：自动检测缺失工具链并按依赖顺序安装（Node → pnpm → Git → Python）；
  - **一键卸载**：确认对话框后批量卸载 Node/Git/Python（npm/pnpm 随 Node 目录一并清理）；
  - 新增 **Python 支持**：官方 python.org 完整安装包静默安装（/PrependPath 写 PATH，UAC 确认）。
- **core/text.rs**：统一子进程输出解码（UTF-8 优先，失败回退 Windows 代码页 GBK/CP1252/CP437，
  按 GetACP 探测），替换全部 20+ 处 `String::from_utf8_lossy` 硬解点。
- **core/pathutil.rs**：用户 PATH（HKCU\Environment）读写/前缀注入/移除（REG_EXPAND_SZ，
  写后广播 WM_SETTINGCHANGE）；`node_dir_injection` 在系统 PATH 无 node 时注入用户级 Node 目录。

### 修复

- **工具链输出日志乱码**（根因：Windows 中文系统 cmd/taskkill/PowerShell 输出 GBK/OEM 字节，
  旧代码按 UTF-8 硬解成 `������`）：全部命令输出解码点（含 stream 逐行、dsh 进程 stdout/stderr、
  npm/git 版本查询、taskkill 错误、PowerShell 下载/安装错误）统一走 core/text.rs 代码页回退解码。
- **分发后其他机器 pnpm 安装失败**（"pnpm 安装失败: 命令退出码: 1"）：
  1. `command::hidden_cmd` 统一把用户级 node_dir 注入子进程 PATH（npm/pnpm/dsh 均为 .cmd，
     运行时需要 node）—— 修复 "'npm' 不是内部或外部命令"；
  2. Node 安装成功后把 node_dir 写入**用户 PATH**（HKCU，免管理员），重启/其他终端全局可用；
  3. `install_pnpm` 在 npm 不在系统 PATH 时回退 node_dir\npm.cmd 绝对路径执行。
- **分发后其他机器 python 安装失败**（"不支持的工具链: python"）：install_toolchain 增加 python 分支。
- **Git/Python 卸载入口匹配安全**：DisplayName 精确语义匹配（Git 仅 "Git"/"Git for Windows"，
  Python 仅主安装项 `Python X.Y.Z (64-bit)`，排除 MSI 拆分组件与 GitHub Desktop/GitHub CLI 误匹配），
  并优先非 MsiExec 卸载器；UninstallString 引号路径 + 参数正确拆分。

### 变更

- 版本同步至 0.4.0（MINOR：新功能工具链管理）。

## [0.3.32] - 2026-09-02

### 变更

- ci(release.yml)：修复“无版本变更提交”时 pwsh 下 `|| echo` 兜底不重置
  `$LASTEXITCODE` 导致 step 误报失败（v0.3.31 首次发布实测）；提交/推送步骤改 `shell: bash`。
- 发布补充便携包：tauri bundler 不产 zip，release 工作流在 tauri-action 后新增
  “打包并上传便携包”步骤（release exe → portable.zip → gh release upload），
  GitHub Release 同时产出安装包（setup.exe）与便携包（portable.zip）。

## [0.3.31] - 2026-09-02

### 修复

- WebView2 内嵌窗口任务栏图标模糊（v0.3.30 修复未生效）二次修复：
  1. 根因定位：旧实现用 `CreateIconFromResourceEx` 从 icon.ico 构造 HICON，
     实测该 API 对本项目 PNG 压缩型 ICO（icon.ico 7 帧均为 PNG）全部返回
     ERROR_INVALID_HANDLE → ICON_BIG 从未成功发送 → 任务栏沿用默认图标模糊；
  2. 改为 tao 同款可靠路径：256px PNG（128x128@2x.png）→ RGBA → `CreateIcon`
     创建原生 256px HICON，进程级缓存持有（不销毁），WM_SETICON 发 ICON_BIG；
  3. 真实运行验证：主窗口与内嵌 Web GUI 窗口 WM_GETICON 读回
     ICON_BIG=256x256、ICON_SMALL=512x512（Windows 高质量缩放到任意任务栏尺寸/DPI）；
  4. 新增 tests/icon_window_test.rs 端到端回归测试（真实窗口 + WM_GETICON 读回断言）。

### 变更（全链路代码审计）

- **工具链状态实时更新**：安装完成后 Rust 广播 `toolchain://changed`，ToolchainPanel 订阅自动刷新；
  安装期间订阅 `install://progress`（channel=toolchain）显示进度条；VersionPanel 进度订阅按
  channel 过滤（npm/github），避免工具链进度污染 dsh 版本安装进度条。
- **修复 dsh .cmd 启动失败**：Windows 上 dsh 是 .cmd shim，`Command::new("dsh")` 直接 spawn 报
  program not found（实测）；process.rs 启动/探测、get_installed_version 均改 `hidden_cmd`（cmd /C 包装）。
- **卸载语义补齐**：卸载前先优雅停止 dsh（ADR-0003）；`keepDshHomeOnUninstall` 开关生效——
  关闭时删除 ~/.dsh（先确认目录含 profiles/settings.yaml 特征，避免误删），默认保留不碰用户数据。
- **补全半成品开关**：`autoStartDsh`（启动器启动时自动拉起 dsh）+ `autoOpenBrowser`（启动成功自动打开内嵌窗口）。
- **工具链检测一致性**：detect 对 npm/pnpm/node 在 PATH 缺失时回退用户级 node_dir（消除“装了 Node 仍报 npm 缺失”矛盾）；
  dsh 启动时若 PATH 无 node 且已装用户级 Node，前缀注入 node_dir（DESIGN §4.3“注入工具链 PATH”）。
- **图标统一**：Rust 创建内嵌窗口时 builder 预置 SMALL 图标（消除创建瞬间默认 exe 图标闪现），
  与主窗口/apply_window_icon 同源（512px icon.png）；窗口图标失败路径落 stderr（不静默吞错）。
- **死代码/重复清理**：commands/logs.rs 复用 core::logging::logs_dir（删重复实现）；detect_node 简化为
  run_cmd 统一回退；删除 11 个无用图标（Square*Logo/StoreLogo/icon.icns，仅 MSIX/macOS 用，本项目 NSIS）；
  generate-icons.ps1 同步：帧数对齐实际 7 帧（16-256）、移除 Square/Store 生成、补 writer/stream Dispose。
- 日志模块顶部注释对齐当前单文件布局。

### 变更（v0.3.30 未独立发布，与 0.3.31 同批合入）

- WebView2 内嵌窗口任务栏图标模糊修复（v0.3.30 方案）：`extract_largest_ico_frame` 构造单帧 ICO 时
  未重写目录项 data offset（仍指向原 ICO 偏移）→ CreateIconFromResourceEx 读到错误偏移
  → 图标损坏/模糊。现正确重写为 22（数据实际位置），附单测 + .NET Icon 实载验证（256px）。
- 主窗口统一走 `apply_window_icon`（SMALL 512px + ICON_BIG 256px），与内嵌窗口一致，
  保证任务栏/标题栏/Alt-Tab/不同 DPI 下图标清晰一致。
- 代码审计清理：
  1. 日志切割（rotate）改为级联轮转（.log.1 → .log.N），修复旧实现重复覆盖丢日志；
     cleanup_old 适配当前单文件布局（原实现只删目录、30 天前的 .log 文件永不清理）。
  2. 死代码移除：core/toolchain 的 dir_exists/user_node_available/system_git_available/
     cleanup_temp、core/stream 的 run_exe、create_desktop_shortcut 的悬空语句。
  3. 重复实现收敛：下载函数统一到 core::toolchain::download；npm prefix 统一到
     core::github::npm_prefix_dir。
  4. process.rs stop 悬垂引用与 restart 死锁风险修复；logging.rs 注释/文档对齐现状。
  5. 移除死依赖 next-themes；删除 Vite 模板残留 favicon（vite.svg/tauri.svg），
     替换为应用图标 favicon.png；清理 icon.ico.bak 与 dist/dist-bundles 构建产物。

## [0.3.29] - 2026-09-02

### 变更

- feat: 日志清屏按钮替代刷新 + 滚动条恢复 + 侧栏默认对齐 340 + emoji 过滤
- 1. 日志工具栏：移除'刷新文件列表'按钮（RefreshCw），新增'清屏'按钮（Eraser）——
- 清空实时流缓冲与文件视图内容；文件列表仍由 30s 定时器自动刷新
- 2. 日志滚动条丢失修复：显式设置 WebView2 scrollBarStyle=default（传统滚动条，
- 非 Fluent overlay，避免 Windows11 overlay 滚动条自动隐藏）；tauri.conf.json
- 主窗口 + 前端内嵌窗口 + CSS 全局 ::-webkit-scrollbar 定制滚动条均生效
- 3. 侧边栏默认宽度对齐右侧：storageKey 升 v3 作废旧拖拽值，左右默认均 340px
- 4. emoji 乱码修复：显示/导出前 strip emoji（U+1F000-U+1FAFF 等区段 +
- 变体选择符 + ZWJ 序列），剔除 dsh 终端彩色 emoji 避免跨编码乱码

## [0.3.28] - 2026-09-02

### 变更

- fix: 日志滚动条紧贴右缘 + npm/GitHub 与日志滚动条统一样式
- 1. 日志滚动条贴最右：移除 Base UI overlay ScrollBar（懒挂载且贴边有偏移），
- viewport 恢复原生滚动条 → 滚动条紧贴 viewport 右缘 = 面板最右缘
- 2. 统一样式：index.css 全局 ::-webkit-scrollbar（10px 圆角 thumb + hover 加深）
- 与 Firefox scrollbar-width/color——npm/GitHub 版本列表（原生 overflow 容器）
- 与右侧日志面板（ScrollArea viewport）滚动条外观完全一致

## [0.3.27] - 2026-09-02

### 变更

- fix: 内嵌窗口任务栏图标取最大帧高清 + 侧栏标题移主内容区 + 日志滚动条贴右不越界
- 1. 任务栏图标模糊（二次修复）：根因是 CreateIconFromResourceEx 传多帧 ICO 时
- 可能选中首帧(16px)放大；改为手动解析 ICONDIR 选最大 256px 帧构造单帧 ICO
- （ICONDIR+entry+PNG），Windows 高质量缩放任意任务栏尺寸
- 2. 左侧栏标题 'dsh-launcher' 移至主内容区 titlebar（收起按钮与右侧控件之间 flex-1），
- 移除侧栏标题与 'dsh-launcher 启动器' 描述；侧栏内容 40px 顶部占位与右侧日志头部对齐
- 3. 日志面板：内容容器右 padding 移除（pl-4）使滚动条贴右缘；ScrollArea min-w-0 +
- viewport overflow-x hidden 修复 flex 子项撑破导致的 1px 横向溢出；
- pre 加 word-break/overflow-wrap + 右留滚动条位，长文本换行不越过滚动条

## [0.3.26] - 2026-09-02

### 变更

- feat: 内嵌窗口任务栏图标高清修复 + 侧栏默认宽 340 + 日志按钮纯图标
- 1. 内嵌 WebviewWindow 任务栏图标模糊根因修复：
- 根因：Windows 任务栏按钮用 ICON_BIG(32px)，tauri set_icon 只设 ICON_SMALL(16px 放大)
- 新增 apply_window_icon：SMALL(512px PNG) + 从多尺寸 icon.ico 用 CreateIconFromResourceEx
- 提取 32px 设 WM_SETICON ICON_BIG；前端创建与 --web-gui 两条路径统一调用
- Cargo.toml 加 windows 0.61 依赖（与 tauri 同版本复用）
- 2. 主窗口左右侧边栏默认宽固定 340px（原左侧 260 / 右侧响应式 360-640）：
- panel-layout 常量改 340；storageKey 升 v2 作废旧拖拽值
- 3. 右侧日志收起按钮去文字，仅保留图标

## [0.3.25] - 2026-09-02

### 变更

- feat: 托盘单击唤起主窗口 + 标题栏控件布局重排 + 仓库 topics
- 托盘：左键单击/双击托盘图标唤起主窗口（show/unminimize/set_focus），菜单改右键弹出（show_menu_on_left_click(false) + on_tray_icon_event）
- 标题栏：修复 space-between 单子元素导致控件组贴左的 bug——窗口控制组（版本号+最小化/最大化/关闭+日志收起）改为贴右靠近日志侧边栏
- 侧栏收起按钮移至主内容左上方（titlebar 左端，sidebar 右缘处）；sidebar-header 仅保留标题
- 仓库 topics 补充：deepseek-harness/dsh/dsh-plugin/ai-agents/electron/typescript/desktop-app

## [0.3.24] - 2026-09-02

### 变更

- ci: bump 脚本 Cargo.lock 正则兼容 CRLF 行尾（修复 CI 检出后行尾转换导致匹配失败）

## [0.3.23] - 2026-09-02

### 变更

- fix: 日志流补全 + dsh 日志获取 + 卸载状态实时刷新 + Release notes 含修改日志
- 任务1: 主窗口操作日志不全修复——uninstall/set_port/set_mirrors/set_switches/set_github_token/create_desktop_shortcut 命令补 logger，操作落盘并推前端实时流
- 任务2: 日志面板获取不到 dsh 日志——list_logs 只扫日期子目录而实际日志在根目录（返回空）；改为兼容两种布局；LogPanel 挂载时读取最新日志文件补流（含启动早期/dsh 日志），避免事件订阅前丢失
- 任务3: 卸载后版本管理安装状态不刷新——新增 version://changed 事件，uninstall/install 成功后广播；VersionPanel/StatusCard 订阅后刷新安装状态与通道状态（安装后仍正常）
- 任务4: dsh 启动命令加 --no-open（不自动弹外部浏览器，由内嵌窗口/快捷方式打开）
- 任务5: 桌面快捷方式确认走 --web-gui 内嵌窗口（实测验证，无需改码）
- 任务6: Release 工作流——CHANGELOG 更新改用按行插入（修复 CRLF 导致条目丢失的 bug）；Release notes 提取本次版本 CHANGELOG 条目（完整修改日志）
- 顺带修复 clippy 存量警告（github.rs/process.rs/tray.rs/logs.rs）

## [v0.3.19] - 2026-09-02

### 变更

- 开源前代码清理：
  - 移除死代码 `github.rs::npm_global_bin_dir_unix`（`#[allow(dead_code)]`）并简化 `npm_global_bin_dir`
  - 删除未使用的 UI 组件 `select.tsx` / `tabs.tsx` / `tooltip.tsx`（无任何引用）
  - 移除死依赖 `next-themes`（`ui/sonner.tsx` 改为由调用方直接传主题，应用本就强制 dark）
  - `package.json` 移除重复的 `tailwindcss` 声明，并同步精简 `package-lock.json`（版本号对齐 + 移除 next-themes 条目）
- 隐私/可移植性：`scripts/generate-icons.ps1` 硬编码的本机绝对路径（`Y:\dsh-launcher\`）改为基于 `$PSScriptRoot` 的相对路径
- `index.html` 标题从 Tauri 模板默认值改为产品名；`README.md` 重写为项目简介（替换脚手架模板）

## [v0.3.18] - 2026-09-01

### 变更

- 卸载按钮添加垃圾桶图标（lucide Trash2），与启动/停止/重启按钮图标风格一致

## [v0.3.17] - 2026-09-01

### 新增

- 单实例（tauri-plugin-single-instance）：整个系统只允许一个启动器进程——
  一个主窗口 + 一个托盘图标；重复启动/双击快捷方式时唤醒已有主窗口
  （显示/还原/聚焦），不再产生多实例
- 单实例回调支持 --web-gui：二次触发桌面快捷方式时在已有实例上
  再打开一个内嵌 Web GUI 窗口（内嵌窗口不限制数量，每次唯一 label）

### 变更

- 左侧边栏描边合并为贯穿整列的单条竖线（app-left::after，从窗口顶到底），
  不再拆分标题栏段与侧栏段；位置跟随侧栏宽度（--sidebar-width），
  与右侧日志描边（贯穿窗口顶）视觉对齐；侧栏收起/移动端时自动隐藏

## [v0.3.16] - 2026-09-01

### 变更

- 卸载按钮从版本管理面板底部移到左侧栏状态卡"重启"按钮右侧，按钮名改为"卸载"
  （悬停提示保留卸载语义与 DSH_HOME 保留说明；未安装时禁用；独立卸载 busy 状态）
- 版本管理卡片自适应窗口高度：Card 填满主区高度，双通道列表区域改为
  随可用高度自适应滚动（列等高、各自内部滚动）
- 左侧栏描边贯穿到窗口顶：标题栏区域在侧栏右缘处补竖线（titlebar::before），
  与 sidebar 的 border-right 无缝衔接，与右侧日志描边（贯穿窗口顶）视觉对齐；
  侧栏收起/移动端时标题栏竖线自动隐藏

## [v0.3.15] - 2026-09-01

### 变更

- 左右侧边栏添加描边（1px，色用现有 --border）：
  - 左侧栏右边缘 border-right、右侧日志面板左边缘 border-left，分隔主区
  - 收起态（宽度 0）自动清除描边，避免残留竖线；紧凑/移动档 overlay 与 drawer 同样生效

## [v0.3.14] - 2026-09-01

### 变更

- 版本管理面板的安装通道改为左右两列排列（大屏 npm 通道 | GitHub 通道 并列，
  窄屏自动单列）：
  - 双通道列表放入 `grid sm:grid-cols-2` 容器（桌面 main≥640px 两列，紧凑档单列）
  - 两通道间原分隔线移除，改为列间距（gap）
- 每通道仅展示最新版本 + 7 个历史版本（列表按版本降序前置 8 条）：
  - npm 通道与 GitHub 通道同步裁剪（`slice(0, 8)`），减少列表滚动

## [v0.3.13] - 2026-09-01

### 变更

- "设置"按钮从标题栏移到左侧边栏底部：侧栏底部用分割线划分为独立区域，
  设置入口与上方（运行状态/工具链）上下分割，标题栏仅保留窗口控制与日志按钮

## [v0.3.12] - 2026-09-01

### 变更

- 右侧日志面板改为整体一体化侧边栏（彻底去除卡片感）：
  - 标题栏去掉背景色（bg-muted/40）与底部描边（border-b），仅保留文字与操作按钮
  - 日志输出容器（ScrollArea）去掉背景（bg-muted/30）、描边（border）与圆角（rounded-md），
    日志内容直接铺满侧栏（无背景色、无描边）
  - 左右两侧栏视觉对仗：均无卡片背景/描边，边框感仅由划分把手（拖拽分隔线）与主区留白提供
- 业务逻辑、按钮样式、选中/悬停交互反馈零改动

## [v0.3.11] - 2026-09-01

### 变更

- 左右侧边栏去除卡片容器，改为全页展示：
  - 左侧栏：dsh 运行状态 / 工具链两张 Card 的容器外壳（背景/边框/圆角/阴影）移除，
    改为标题块（CardTitle/CardDescription 样式保留）+ 分隔线 + 内容区直接铺满侧栏宽
  - 右侧日志面板：去除子窗口外观（rounded-lg/border/bg-card/shadow-sm），
    直接全页贯穿展示
- 业务逻辑、按钮/输入/徽标等全部视觉样式与功能零改动

## [v0.3.10] - 2026-09-01

### 变更

- 主窗口改为无边框（decorations: false）：窗口内容标题栏提升为唯一主窗口标题栏，
  与系统原生标题栏融为一体；标题栏 `data-tauri-drag-region` 支持拖拽移动窗口，
  双击空白处最大化/还原
- 标题栏右上角自绘窗口控制按钮（最小化/最大化/还原/关闭，补 capabilities 权限：
  start-dragging/minimize/toggle-maximize/unmaximize/close/is-maximized）；
  右侧日志面板从窗口顶部贯穿到底（回归 v0.3.3 覆盖标题栏右侧区域的视觉），
  标题栏与侧边栏同背景无分隔线融合贯穿
- 日志收起/展开按钮从日志面板内部移到主窗口标题栏最右端（关闭按钮右边），
  收起后按钮自动变为"日志"展开入口；新增 WindowControls 组件

### 修复

- 修复无边框窗口控制组件在非 Tauri 环境崩溃：getCurrentWindow() 惰性获取
  （useEffect 内 + ref 存储），浏览器预览时不崩溃

## [v0.3.9] - 2026-09-01

### 变更

- 前端布局重构为三栏架构（复刻 pi-agent-desktop 布局模型，仅改布局/排版，
  视觉样式与业务功能零改动）：
  - Left Sidebar（dsh 运行状态 + 工具链）| Main（标题栏 + 版本管理）| Right Panel（日志）
  - 左右栏独立展开/收起，收起后 Main 自动扩展，再展开恢复之前宽度
  - 拖拽调整宽度（pointer → min/max clamp → CSS 变量实时应用 → localStorage 持久化），
    宽度状态与 open/close 状态分离；窗口缩放时重新 clamp 防溢出
  - Sidebar 默认 260px（min 180 / max 480）；Right Panel 默认响应式（视口 42%，
    min 300 / max 1200），左侧栏/右栏互不影响地独立计算可用空间
  - 响应式：≥960px 三栏；641~959px 两栏（Right Panel 改 fixed overlay 不参与 split）；
    ≤640px 移动端（Sidebar 变 drawer + 遮罩，Main 占满，Right Panel 全屏 overlay）

### 新增

- 新增 src/lib/panel-layout.ts（宽度常量/clamp/响应式计算）、
  src/hooks/useResizablePanel.ts（拖拽 resize + 持久化）、
  src/hooks/useIsMobile.ts（≤640 断点）、src/components/AppShell.tsx（三栏骨架）；
- StatusCard/ToolchainPanel 内部行加 flex-wrap（仅布局属性，窄宽下不溢出）

## [v0.3.8] - 2026-09-01

### 修复

- 修复关闭内嵌 Web GUI 窗口导致启动器（含 dsh）也被退出的 bug：
  - 根因：窗口关闭事件处理（handle_window_event）未区分窗口，内嵌窗口
    关闭被误当主窗口关闭——开关 1 开时触发 `app.exit(0)` 连带退出启动器；
    开关 1 关时被 `prevent_close` 拦截只能隐藏、无法真正关闭
  - 修复：仅主窗口（label="main"）应用关闭/最小化到托盘策略；
    内嵌 Web GUI 窗口（label 前缀 `dsh-web-gui-`）关闭直接放行、
    最小化不受影响，与启动器互不干涉

## [v0.3.7] - 2026-08-31

### 新增

- 标题栏新增"设置"按钮：点击打开设置弹出子窗口（Dialog），
  设置面板从页面内嵌改为模态弹窗（embedded 模式去 Card 外壳）；
  主区右列仅保留版本管理

## [v0.3.6] - 2026-08-31

### 修复

- 修复日志超宽不换行：日志内容区加 `break-words`（overflow-wrap），
  超长无空格内容（URL/路径/错误堆栈）在容器宽度内自动断行
- 修复 taskkill 错误日志乱码：
  - 根因：Windows 中文系统 taskkill stderr 为 GBK 编码，`from_utf8_lossy`
    按 UTF-8 解码产生 `����` 乱码
  - 修复：新增 `decode_console_text`（UTF-8 优先，失败按 GBK 解码，基于
    encoding_rs），应用于 taskkill/卸载等系统命令输出；新增单元测试

## [v0.3.5] - 2026-08-31

### 修复

- 优化退出/关闭窗口太慢：
  - 根因：stop 等待循环依赖 child 句柄 try_wait，但句柄已被监视线程 take 走
    → 恒不退出 → 总是等满超时；且 node/pnpm 进程树对无 /F 的 taskkill 不响应
  - 修复：改用 tasklist 探测进程存活（process_alive），优雅等待缩短到 1 秒，
    未退出立即升级 /F 强杀；实测停止耗时 6.1s → 1.8s
- 修复退出驻留 dsh 后重开启动器无法管理（停止/重启）：
  - 根因：ProcessManager 状态在内存，重开进程状态丢失（Stopped），
    且 pid 未知无法停止
  - 修复：启动时探测配置端口，若 dsh 在监听则 `adopt_running` 恢复 Running 状态
    （端口/URL 一并恢复）；stop 时 pid 未知则按端口查 PID（Get-NetTCPConnection）
    再终止

## [v0.3.4] - 2026-08-31

### 变更

- 日志侧边栏视觉优化：
  - 展开时呈现独立"子窗口"外观（边框/圆角/背景 + 标题栏，内容区独立）
  - 收起时右侧保留贯穿全高的窄条（顶部展开按钮 + 竖排标签，带背景分隔）

## [v0.3.3] - 2026-08-31

### 变更

- 日志侧边栏贯穿到标题栏：从窗口顶部（标题栏同排）延伸到底部全高，
  覆盖标题栏右侧区域；主区（头部 + 两列内容）在左侧，布局更紧凑

## [v0.3.2] - 2026-08-31

### 修复

- 修复内嵌窗口手动关闭后无法再次唤起：
  - 根因：固定 label 的窗口关闭后 `getByLabel` 返回残留对象，setFocus 静默失败
  - 修复：每次打开用唯一 label（时间戳），彻底避免残留冲突；创建错误经
    `tauri://error` 事件提示

### 变更

- 桌面快捷方式改为打开内嵌窗口（而非外部浏览器）：
  - 创建 .lnk 指向 dsh-launcher.exe + `--web-gui` 参数（WScript.Shell）；
  - 启动器收到 `--web-gui` 参数时自动打开内嵌 Web GUI 窗口（URL 从日志提取 token）

## [v0.3.1] - 2026-08-31

### 修复

- 修复内嵌 Web GUI 窗口关闭后无法再次打开：
  - 根因：窗口关闭后 `getByLabel` 可能返回残留对象，`setFocus` 对已销毁窗口
    静默失败 → 永远聚焦不存在的窗口，无法重建
  - 修复：`getByLabel` 返回对象时先尝试 setFocus，失败则 destroy 残留后重建；
    监听 `tauri://destroyed` 确保下次可正常重建

### 新增

- 内嵌窗口"发送到桌面"：创建桌面快捷方式（.url 文件），
  双击用默认浏览器打开 dsh web（含 token 认证）；新增 `create_desktop_shortcut` IPC

## [v0.3.0] - 2026-08-31

### 修复

- 修复日志侧边栏收起按钮丢失：
  - 根因：收起按钮为纯图标（X），在窄边栏标题栏中被压缩/不易察觉
  - 修复：收起按钮改为带文字的 outline 按钮（"收起"），更显眼；
    标题栏精简（去掉重复的"实时流"文字徽标），避免按钮被挤出

## [v0.2.9] - 2026-08-31

### 修复

- 修复内嵌 Web GUI 仍报 `dsh web authentication required`：
  - 加固 token URL 获取链路：内存捕获失败时从最新日志文件兜底提取
    （dsh stdout 已实时落盘，含 `http://127.0.0.1:<port>/?token=` 行）；
    前端打开时轮询等待 URL 捕获（最多 3 秒），避免启动初期打开用裸 URL
  - 实测确认：curl 模拟浏览器（token URL → 303 → 根路径 200）认证链路正常，
    问题在 URL 获取时机，已通过轮询 + 日志兜底修复

### 变更

- 日志侧边栏去卡片化：固定在右侧（无 Card 容器边框/背景），
  标题栏 + 内容区直接作为侧边栏；收起时右侧窄条按钮

## [v0.2.8] - 2026-08-31

### 修复

- 修复内嵌 Web GUI 打开报 `dsh web authentication required`：
  - 根因：dsh web 采用启动 token 认证（URL 带 `?token=`），内嵌窗口用裸 URL
    访问 → 401
  - 修复：启动器从 dsh stdout 捕获带 token 的完整 URL（`get_web_url` IPC），
    内嵌窗口与外部浏览器均用完整 URL 打开；新增 extract_web_url 单元测试

### 变更

- 日志卡片改为右侧边栏：可展开/收起（收起时右侧显示窄条展开按钮），
  展开时固定宽度内部滚动，主区两列布局

## [v0.2.7] - 2026-08-31

### 新增

- 启动探活与自动修复：dsh 启动后端口监听 → 状态转为运行中；
  若进程启动即退出（端口未监听），自动检测并卸载不兼容插件（dshmarket 与
  dsh-settings API 不兼容导致 alpha.2 无法启动），日志记录修复过程

### 修复

- 修复安装 dsh-v0.1.2-alpha.2 后无法启动：
  - 根因：web profile 的 dshmarket@1.36.0 插件引用 `@deepseek-ai/dsh-settings` 已移除的
    `installSettingsSection` 导出 → dsh web 启动即崩溃
  - 修复：启动器启动后探活，检测到启动即崩时自动执行
    `dsh plugin --profile web uninstall dshmarket` 修复并提示重试

### 变更

- 日志面板：实时流最新条目排最上（自动滚动到顶部）；日志卡片为独立滚动容器
  （自带滚动条，不随页面滚动）

## [v0.2.6] - 2026-08-30

### 变更

- 窗口尺寸调整：主窗口默认 1600×900（最小 1280×800）；内嵌 Web GUI 窗口同步 1600×900
- 布局防溢出：页面容器适配 1600 宽度（max-w-[1560px]），三列 min-w-0 防水平溢出；
  日志列固定视口高度（内部滚动）不再撑破窗口边界；卡片高度对齐保持

## [v0.2.5] - 2026-08-30

### 新增

- 自定义 GitHub API Key（Settings → GitHub Token）：
  - 配置 GitHub Personal Access Token，git ls-remote / git clone 通过
    `http.extraheader` 注入认证头（token 不进 URL/日志），避免 GitHub 限流
  - 新增 `set_github_token` IPC

### 修复

- 修复停止/重启 dsh 无效：
  - 根因：监视线程 `take()` 移走 Child 句柄后，stop 依赖 `self.child` 取 PID →
    恒为 None → 不执行 taskkill，dsh 继续运行；重启因端口残留占用而失败
  - 修复：ProcessManager 独立记录 PID（`pid` 字段），stop 用 PID + `taskkill /T`
    杀进程树（实测可杀掉 node 长驻进程）；重启时清理 PID/端口再启动
- 修复内嵌原生窗口按钮实际打开外部浏览器：
  - 根因：capability 缺少 `core:webview:allow-create-webview-window` 等权限，
    WebviewWindow 创建被拒
  - 修复：capabilities 增加 webview/window 创建、显示、聚焦等权限
- 修复外部浏览器按钮无反应：
  - 根因：`window.open` 在 Tauri 沙箱内被拦截
  - 修复：改用 `@tauri-apps/plugin-opener` 的 `openUrl`（opener:default 权限）

## [v0.2.4] - 2026-08-30

### 变更

- 界面紧凑化：全局按钮/输入框/徽标缩小一号（Button h-7/6、Input h-7、Badge h-4）
- 卡片标题与描述精简（工具链→依赖、设置→镜像源与运行行为等），表达更紧凑
- Web GUI 面板整合进 dsh 运行状态卡片（内嵌/外部浏览器按钮并入状态区）
- 三列布局调整：左（状态+Web GUI/工具链）、中（版本管理/设置）、右（日志独立列，
  占满高度），列等高对齐不凹凸
- 滑动开关：描述简化（仅保留开关标签），六个开关在卡片内两列排布，尺寸改 sm

## [v0.2.3] - 2026-08-30

### 修复

- 修复安装完成后启动失败 `program not found`：
  - 根因：GitHub 通道安装仅 clone + build，dsh 从未加入全局 PATH → 启动器 `dsh web` 找不到命令
  - 修复：启动逻辑优先 PATH 的 dsh，找不到时改用安装目录内 `pnpm dsh web --port <p>`
    （cwd=安装目录，与手动流程一致）；安装完成后在 npm 全局目录创建 dsh.cmd shim，
    `dsh` 命令全局可用
- 修复安装完成后安装状态不实时更新：
  - 根因：`get_installed_version` 依赖 `dsh --version`（PATH 查找），GitHub 安装后 dsh 不在 PATH → 恒为未安装
  - 修复：优先检测 GitHub 安装目录（本地文件系统），再尝试 PATH 的 dsh --version；
    GitHub 目录返回 `github:<version>`，npm 返回 `npm:<version>`

### 变更

- GitHub 克隆目录固定为 `%LOCALAPPDATA%\dsh-launcher\github-dsh\deepseek-harness`
  （不再按版本号命名，全局单版本覆盖同一目录）
- 界面改三列布局：左列（状态控制/工具链）、中列（版本管理/Web GUI）、右列（设置/日志）
- 版本管理面板新增：安装状态独立刷新按钮 + harness 下载/安装目录显示
  （GitHub 目录 + npm 全局目录）

## [v0.2.2] - 2026-08-30

### 修复

- 修复安装进度条长时间为 0（github 下载 / pnpm build 开始阶段）：
  - 根因：git clone 的进度输出以 `\r` 结尾持续刷新同一行（无 `\n`），
    `BufRead::lines()` 按 `\n` 切分 → 整个 clone 期间不产生行 → 进度回调不触发；
    pnpm install/build 则完全没有行回调，开始阶段进度停在 0
  - 修复：core/stream.rs 改为逐字节缓冲、`\r` 与 `\n` 均视为行分隔
    （`read_all_lines`），git 每条进度刷新实时回调；
    pnpm install/build 增加按输出行数步进的进度回调（install +2%/行、build +1%/行，
    95% 封顶避免提前满）；npm 通道步进从 5% 调整为 2%/行
  - 新增 read_all_lines 单元测试（真实 git 输出片段、\r\n 混合、普通换行）

## [v0.2.1] - 2026-08-30

### 修复

- 修复读取 GitHub 版本列表报 `curl: (22) ... 403`（GitHub API 限流）：
  - 根因：GitHub API 未认证限流 60 次/小时，403 时整个列表不可用
  - 修复：`list_releases` 改用 `git ls-remote --tags` 作为主路径
    （git 走 HTTPS 协议不受 API 限流影响，1~2 秒返回全部 tag）；
    镜像配置时仍走镜像 URL
  - 附带收益：API /releases 只含 release，ls-remote 能拿到全部 tag（含 rc），
    版本列表更全；新增 `parse_tags_from_ls_remote`（去重/去剥离引用/降序）+ 单元测试

## [v0.2.0] - 2026-08-30

### 修复

- 修复刷新 GitHub 版本列表报"GitHub API 返回非数组"：
  - 根因：curl `-s` 不检查 HTTP 状态码，GitHub API 限流（403）时响应是
    `{"message":"API rate limit exceeded..."}` 对象而非数组，JSON 解析成功但
    `as_array()` 失败 → 报笼统的"返回非数组"
  - 修复：curl 改用 `-sS -fL`（HTTP 4xx/5xx 即失败）+ 追加 HTTP 状态码；
    失败/非数组时提取响应体 `message` 字段给出可操作原因
    （限流提示 60 次/小时、可配置镜像源），不再笼统报错

### 新增

- `extract_api_error_message`：从 GitHub API 错误响应提取 message（含限流中文提示），
  配套单元测试

## [v0.1.9] - 2026-08-30

### 变更

- 版本管理：读取 npm / GitHub 通道版本列表失败或结果为空时，写入启动器日志
  （落盘 + 推送到日志面板实时流），便于定位网络/镜像源问题

## [v0.1.8] - 2026-08-30

### 新增

- 版本管理面板：npm 通道与 GitHub 通道各新增独立"刷新版本列表"按钮，
  点击仅重新拉取对应通道的 harness 版本列表（刷新中图标旋转），互不阻塞

## [v0.1.7] - 2026-08-30

### 修复

- 修复 GitHub 通道 v0.1.2-alpha.1 安装无响应/假成功：
  - 根因：pnpm install/build 误用 `hidden_cmd("")`（`cmd /D /C ""` 退出码 0 但什么都不执行），
    dsh 依赖从未安装、从未构建 → 安装"成功"但 dsh 不可用
  - 修复：改为 `hidden_cmd("pnpm")`，真实执行 pnpm install + pnpm run build
- 修复日志无实时流式输出：
  - process.rs 此前"先读完全部 stdout 再读 stderr"，若 stdout 长期无数据且进程存活，
    stderr 输出永远不会被读取 → dsh 日志缺失；改为双线程并行读取 stdout/stderr
  - 日志面板自动滚动失效（滚动容器是 ScrollArea viewport，非内层 div）；
    ScrollArea 增加 viewportRef 透传，实时流新行自动滚动到底
- 修复安装过程无日志：安装（npm/GitHub 通道、工具链）改用流式执行（core/stream.rs），
  每行输出实时写日志（落盘 + 前端 `log://line` 事件）

### 新增

- 安装进度条：新增 `install://progress` 进度事件（core/events.rs），
  版本管理面板显示阶段（准备/下载/安装依赖/构建）+ 百分比 + 消息；
  git clone 解析真实下载百分比，npm/pnpm 按输出行数阶段估算
- 新增 core/stream.rs 流式命令执行器（双线程读管道、逐行写日志、行回调推进度）

### 变更

- 工具链安装（Node/Git/pnpm）同步接入日志流与进度事件
- 流式输出清理行尾 `\r`（git/pnpm 进度行的回车符不再污染日志）

## [v0.1.6] - 2026-08-29

### 修复

- 修复任务栏图标模糊：
  - 主窗口显式设置高清图标（`window.set_icon`，嵌入 512px icon.png，DPI 缩放后仍清晰）
  - 重生成多尺寸 icon.ico（16/24/32/48/64/128/256），bundle.icon 增加 512px icon.png
    （exe 资源图标与任务栏/Alt-Tab 均取高清源，不再模糊）
- 修复 GitHub 通道安装失败：
  - 根因：GitHub release tag 实际命名是 `dsh-v0.1.2-alpha.1`（带 dsh- 前缀），
    但 `install_version` 对非 v 开头的版本号自作主张加 v 前缀，导致
    `--branch vdsh-v0.1.2-alpha.1` → branch not found
  - 修复：`list_releases` 返回的 tag_name 即完整 tag，`install_version` 原样使用
  - 顺带修复前端排序：`versionSortDesc` 现能正确处理 dsh- 前缀（提取到 src/lib/version.ts）

### 变更

- 页面布局改为两列：左列（状态控制 / 工具链 / Web GUI），右列（版本管理 / 设置 / 日志），
  列内用分割线分隔
- GitHub 安装成功提示不再拼接多余 v 前缀（`GitHub 通道 dsh-v0.1.x 构建完成`）

### 新增

- 测试：GitHub 真实 tag 验证（dsh-v0.1.2-alpha.1）、目录路径构造

## [v0.1.5] - 2026-08-29

### 修复

- 修复托盘图标丢失：TrayIconBuilder 未设置 `.icon()`；改为编译期嵌入 32x32.png
  （`include_bytes!` + `Image::from_bytes`，启用 `image-png` feature），无运行时路径依赖
- 修复启动端口输入框默认显示 0：`AppConfig::Default` 改为 port=3080
  （原 derive Default 使 u16 默认 0）；前端读取旧配置 port=0 时兑底重置为 3080

### 变更

- 界面整合为单页：移除 Tabs，所有功能（状态/工具链/Web GUI/版本管理/设置/日志）
  以 `<Separator>` 分割线纵向排列在一页
- 版本管理 NPM/GitHub 通道均以最新版本置顶排序（语义化版本号比较，降序）
- Web GUI 面板端口从配置读取（不再硬编码 3080），显示当前端口徽标

### 新增

- 测试：配置默认值（端口 3080）、托盘图标 PNG 可被 tauri Image 解析

## [v0.1.4] - 2026-08-29

### 变更（并发架构）

- 所有阻塞型 IPC 命令改为 async + `spawn_blocking`（不占 Tauri IPC 线程，互不阻塞）：
  - `commands/dsh.rs`：启动/停止/重启
  - `commands/version.rs`：版本列表/已装版本/安装/卸载
  - `commands/toolchain.rs`：工具链检测/一键安装
- `ProcessManager` 增加 `op_lock` 生命周期操作互斥锁，防止并发双 start/双 stop 竞态；
  restart 改为无锁版内部调用（避免重入死锁）
- 托盘菜单启动/停止/重启改为异步 spawn（原同步 stop 的 ≤5s 等待会卡托盘事件循环）
- 窗口关闭（直接退出模式）与托盘退出改为异步：先后台停止 dsh，完成后再退出应用
- 前端 VersionPanel 改 `Promise.allSettled` 并发拉取（npm/GitHub/已装版本互不阻塞）

### 新增

- 并发回归测试 tests/concurrency_test.rs：验证 3×300ms 长任务并发执行（实测 300ms，串行应 900ms）

## [v0.1.3] - 2026-08-29

### 修复

- 修复版本管理无法列出 deepseek-harness 版本、无法安装 dsh 的 bug：
  - 根因：`DETACHED_PROCESS` 标志导致 npm.cmd（batch 脚本）的子进程输出管道失效，
    `npm view` / `npm install` 成功退出但 stdout 为空；且 `Command::new("npm")` 在
    Windows 上找不到 .cmd 可执行文件
  - 方案：`command::hidden` 移除 `DETACHED_PROCESS`（仅保留 `CREATE_NO_WINDOW`）；
    npm/pnpm 统一经 `cmd.exe /D /C` 包装（`command::hidden_cmd`），按 PATHEXT 正确解析 .cmd
- 修复 总览/版本管理/设置 Tabs 来回切换未响应的 bug：
  - 根因：base-ui Tabs 受控模式（value + onValueChange）的 React state 竞争
  - 方案：改为非受控 `defaultValue`（官方推荐模式）

### 新增

- 集成测试 tests/npm_versions_test.rs：验证 npm view / npm install 在无窗口 flags 下输出正常
- Edge headless CDP 冒烟测试脚本验证 Tabs 切换（临时脚本，未入库）

## [v0.1.2] - 2026-08-29

### 新增

- 图标统一替换：所有图标（14 个 PNG + icon.ico 多尺寸）由 `src-tauri/icons/ico.png` 生成
- 新增 `scripts/generate-icons.ps1` 图标生成脚本（可重复执行）

### 变更

- 所有子进程（dsh/npm/git/pnpm/curl/taskkill/powershell）统一应用 `CREATE_NO_WINDOW + DETACHED_PROCESS`，
  不再弹出黑色命令提示符窗口（新增 `core/command.rs` 统一封装）
- `tauri.conf.json` 图标配置移除 `icon.icns`（Windows 项目不需要 macOS 格式）

## [v0.1.1] - 2026-08-29

### 新增

- 工具链真实安装：
  - Node：官方 zip 解压到用户级目录（`%LOCALAPPDATA%\dsh-launcher\toolchain\node`），免管理员
  - Git：官方安装包 + runas 提权（UAC）静默安装
  - pnpm：npm i -g
  - 支持镜像源（npm registry / GitHub 加速 / Node 二进制）
- GitHub 通道真实实现：releases API 查询（含镜像）、clone + pnpm install + build、卸载清理
- 系统托盘：菜单（打开主窗口 / 启动 / 停止 / 重启 / 退出），退出先优雅停止 dsh
- 窗口行为：关闭按钮直接退出（含 dsh）/ 最小化到托盘 滑动开关
- 配置联动：端口 / 镜像源 / 6 个滑动开关 持久化到 `%APPDATA%\dsh-launcher\config.json`
- 设置面板（镜像源预设 + 滑动开关）与版本管理面板（双通道列表 + 安装/卸载）
- 日志实时流式推送（Tauri event `log://line`，前端实时接收 + 自动滚动 + 导出）
- 单元测试：日期算法（date_to_days / is_older_than / epoch_to_date）

### 修复

- 日期转天数算法偏移错误（JDN 未减 Unix 纪元偏移 2440588）

## [v0.1.0] - 2026-08-29

### 新增

- 工程骨架：Tauri 2 + React + TypeScript + shadcn/ui（黑暗主题）
- Rust 后端核心模块：
  - `core/process`：dsh 进程生命周期（启动/停止/重启），SIGTERM 优雅排空 + 5 秒超时强杀
  - `core/port`：端口探测（启动前预检 + 运行状态兜底）
  - `core/config`：启动器配置持久化（`%APPDATA%\dsh-launcher\config.json`）
  - `core/logging`：全域日志落盘（按天轮转 + 10MB 切割 + 保留 30 天）
  - `commands/dsh`：生命周期 IPC（启动/停止/重启/状态）
  - `commands/toolchain`：工具链检测（Node/npm/pnpm/Git/Python）与一键安装（pnpm 已实现，Node/Git 占位）
  - `commands/version`：双通道版本管理（npm 通道可查询/安装，GitHub 通道占位）
  - `commands/logs`：日志文件列表/读取/导出
- 前端界面：
  - 状态卡片（运行状态徽标 + 启动/停止/重启 + 端口配置）
  - 工具链检测面板（一键安装）
  - Web GUI 面板（Tauri WebviewWindow 内嵌 + 外部浏览器兜底）
  - 日志面板（文件列表 + 实时读取 + 导出）
- 三处版本号同步（package.json / Cargo.toml / tauri.conf.json）= 0.1.0
- NSIS 安装包（简体中文）+ 便携 zip 分发配置

### 设计文档

- `CONTEXT.md`：术语表（dsh、DSH_HOME、运行目录、安装通道、工具链）
- `docs/DESIGN.md`：设计总览（架构、模块划分、核心流程）
- `docs/adr/`：ADR-0001~0004（双通道模型、生命周期、单版本模型、版本号策略）


