# CONTEXT — dsh-launcher 词表

> 本文件是**词表（glossary）**，不是 spec，不记录实现细节。实现决策见 `docs/adr/`。

## 核心名词

- **dsh（deepseek-harness）**：DeepSeek AI 开源的 agent harness，`@deepseek-ai/dsh`。以 `dsh web` 启动 Web GUI，默认 `http://127.0.0.1:3080`。
- **dsh-launcher（本产品）**：管理 dsh 及其工具链的 Windows 桌面启动器。本身不是 dsh 的一部分，不修改 dsh 源码。
- **运行目录（workspace root）**：启动 dsh 时所在目录，作为默认 workspace 根目录。由用户在启动器里配置，每次启动可改。
- **DSH_HOME**：dsh 自管的数据目录（`profiles/<name>`、`settings.yaml`、`.credentials.yaml` 在其中）。启动器**只读展示、不接管**。
- **profile**：dsh 的具名应用组合，如 `web`、`headless`、`sdk`、`acp`。`dsh web` 是 `--profile web` 的别名。启动器主控 `web` profile。
- **工具链（toolchain）**：dsh 运行依赖的可执行环境：Node.js、npm、pnpm、Git，以及可选检测项 Python。全部**全局安装、单版本**，无用户级目录。
- **安装通道（install channel）**：dsh 的来源。两个：**GitHub 源码通道**（v0.1.2-alpha.1 及更新版本，clone + pnpm 构建）与 **npm 通道**（registry 现有版本，当前最新 0.1.1-rc.2）。全局同时只有一个激活版本，切换 = 先卸载再装（ADR-0003）。
- **版本目录（version dir）**：全局安装的 dsh 版本目录。npm 通道与 GitHub 通道产物不同，各自独立；全局仅激活一个。

## 状态

- **dsh 运行状态**：`running` | `stopped` 等五态（见 DshStatus）。判定 = 本启动器托管进程事件 + 收养实例的**进程身份匹配**（端口监听者命令行需形如 dsh，防误杀无关进程）+ 每 5 秒后端兜底对账（v0.4.13）。
- **启动就绪收敛**：`starting → running` 由启动探活线程（8 秒内快速识别"启动即崩"并归因插件）与 5 秒对账线程**共同**保证；探活超时**不等于失败**，对账持续探测至端口就绪（v0.9.1 修复，此前超时后状态永久卡 `starting`）。
- **托管实例 / 外部实例**：托管 = 本启动器 `spawn` 的子进程（pid != 0）；外部 = 启动器**收养**的他人启动的 dsh（pid == 0）。dsh 的访问 token 是**进程级随机数**，仅从该进程 stdout 打印，故外部实例的 token 启动器**原理上不可得** → 内嵌打开前须询问用户是否**接管**（停止后由启动器重新拉起）。
- **工具链状态**：`present`（版本满足）| `missing`（不存在）| `mismatch`（存在但版本不符）。可检测、可一键安装/升级、可配镜像源。
- **工具链安装模式**：Node = 官方 zip 解压到**用户级目录** `%LOCALAPPDATA%\dsh-launcher\toolchain\node` 并写入用户 PATH（免 UAC）；Git/Python = 官方安装器（提权，可能触发 UAC）。npm/pnpm 随 Node zip 自带于同一目录。

## 运行目录

- **workspace root**：启动 dsh 时的 CWD，由 dsh 官方行为决定（默认该目录）。启动器**不配置、不干预、不校验**。

## 插件（ADR-0005）

- **受管 profile**：启动器唯一管理的 profile，固定为 `web`（`dsh web` 的官方别名目标）。
- **插件（profile bundle）**：`dsh.profile.bundles` 里的一层，由 `dsh plugin` 官方通道装卸。
- **插件状态**：`uninstalled`（依赖不存在）| `plain`（普通依赖，不声明 `dsh.bundle`，不可启停）| `enabled` | `disabled`。
- **启停**：写 profile 的 `cordis.patch.yml` 受管区块（`- id:` + `disabled:`），由 dsh `live` 热重载，**不需要重启**。
- **装卸**：走 `dsh plugin --profile web add/remove`，属于 bundle 成员变更，**必须重启 dsh**（启动器自动完成）。
- **受管区块（managed block）**：启动器在 `cordis.patch.yml` 中拥有的一段（marker 包裹），块外内容逐字节保留。
- **upstream 插件 / 自研插件**：`upstream` 参与自动同步（npm 版本或 git commit）；`in-house`（本地路径/link/tarball）**永不被同步任务改动**。

## 技能共享（ADR-0005）

- **共享真源（canonical store）**：**官方 `agentsHome` 根**（默认 `~/.agents`）——技能 = `~/.agents/skills`（官方 `skill-filesystem` 的 `user-agents` 根，rank 500）；指令 = `~/.agents/AGENTS.md`；词表 = `~/.agents/CONTEXT.md`（dsh 不读）。**不再是 `~/.agents/agent`**（ADR-0006 D17 修订）。
- **共享模式**：`link`（`<dshHome>/AGENTS.md` 链接到真源；技能**无需链接**，由官方 rank 500 原生覆盖）| `config`（写 `$DSH_HOME/cordis.patch.yml` 的 shared 区块，让 dsh 直接读真源）。
- **资源状态**：`missing` | `linked` | `config` | `conflict`（真实文件/目录，绝不自动删除）| `broken`（链接指向别处或**断裂**）| `native`（该资源**不需要**链接，真源已由官方扫描根原生覆盖）。
- **需要链接判定（`needs_link`）**：由官方事实决定 —— `agents-md` **需要**（官方固定读 `<dshHome>/AGENTS.md`）；`skills` 与 `context-md` **不需要**（技能走官方 rank 500 根；dsh 不读 `CONTEXT.md`）。判定链顺序为「链接 → 真实文件 → 原生根」，因此视图侧遗留的**断链**不会被误判成 `native`。
- **链接修复动作（`skill repair-links`）**：一次性、**可幂等重跑**的迁移动作 —— 修复需要链接的资源；清理**启动器自己创建的断链**（目标落在 `agentsHome` 下的）；保留一切含用户内容的真实文件/目录，以及指向 `agentsHome` **之外**的用户自建链接。绝不删除用户内容。
- **白名单唯一例外（失败回滚）**：官方无"回退到任意历史 lock 状态"的能力，故 `package.json` / `pnpm-lock.yaml` / `pnpm-workspace.yaml` / `cordis.patch.yml` 仅允许在**失败回滚**时由备份还原写入（`core/plugin/mod.rs::rollback_after_failed_official_op` **一处**），且其后**必跟**一次官方通道 `dsh plugin … install` 收敛（ADR-0006 D18）。

## 技能管理（ADR-0007）

- **技能（skill）**：官方 `skill-filesystem` 的发现单元 —— 扫描根**顶层**的 `<name>/SKILL.md` 目录包，或 `<name>.md` 平铺文件。官方**刻意不发现嵌套的 `**/SKILL.md`**（只发现一层）。技能目录自身即资源基（`resourceBase`）：正文可按相对路径引用同目录的兄弟文件（`CONTEXT-FORMAT.md`、`ADR-FORMAT.md`、`tests.md` 等），故技能是**目录**，不是单个文件。
- **技能根（skill root）**：官方六个扫描根及优先级 rank —— 项目 `.dsh/skills`(100) / 项目 `.agents/skills`(200) / custom(300) / 用户 `<DSH_HOME>/skills`(400，忽略 `.system`) / 用户 `<agentsHome>/skills`(500) / bundled(600，真实安装下不存在)。**项目根取决于 dsh 会话的工作区**（非进程 CWD），启动器不可知；custom 根由 preset 声明且不出现于合成配置。故启动器**只管理 400 与 500 两个用户级根**。
- **模型可调用（modelInvocable）**：frontmatter `disable-model-invocation` 的取反。为 false 时技能不进模型可见目录、`skill` 工具不暴露，但**仍可由人在 dsh GUI 输入 `/名称` 调用**。
- **用户可调用（userInvocable）**：frontmatter `user-invocable`。为 false 时技能不进 `/` 建议菜单。
- **停用（disable）**：本产品的单一开关，语义 = 写 `disable-model-invocation: true`。**停用 ≠ 不可用** —— 用户仍可 `/名称` 手动调用；停用只关闭官方两个调用面中的「模型面」。
- **技能启用状态**：`unset`（无该键 = 官方默认允许模型调用）/ `enabled` / `disabled` / `conflict`（重复键或非规范布尔值 —— 只读，拒绝操作）。
- **生效技能 / 被同名覆盖技能**：同名技能按 rank **数值大者生效**；被覆盖者仍在磁盘上仍被加载，但停用它对模型无影响。
- **技能来源**：`local`（本地创建或来历不明）/ `imported`（由 git URL 导入，记有来源 URL 与 commit）。
- **导入（import）**：把 git 仓库中递归收集到的技能**扁平化**到用户级技能根的 `<name>/`；目标目录名取 frontmatter 的 `name`，而非仓库内的目录名。
- **来源记录（skill source record）**：记录「哪个技能来自哪个 URL 的哪个 commit」的元数据，供人工核对与**手动**检查更新。**不驱动任何自动更新。** 落盘文件名固定为 `skill-sources.json` —— **不得**用 `skills.json`，该名字已被退役的共享模块占用（存 `preferred_mode`/`last_applied`）。
- **技能共享（已退役）**：原 ADR-0005 的 link / config 两种共享模式及其迁移动作在本产品中退役 —— 技能走官方 rank 500 原生覆盖，本不需要链接。前端入口已移除；Rust 后端保留供 CLI（`skill status|apply|migrate|repair-links`）作应急路径。

## 技能导入与更新（ADR-0008）

- **导入（import）**：把任意 git 仓库**浅克隆到临时目录**，**递归收集所有 `SKILL.md`**，逐个校验（`name`/`description` 必需、`name` 满足官方 `isSkillName`）后**扁平化**到 `<agentsHome>/skills/<name>/`。**整目录复制**（含兄弟资源文件与 `agents/` 子目录）—— 技能是目录，只复制 `SKILL.md` 会断引用。目标目录名取 frontmatter 的 `name`，不取仓库目录名。
- **文件级覆盖（Q29 A）**：导入/更新时上游提供的文件覆盖、**本地独有文件/目录保留**、**上游已删除的文件不落地删除**。应用前有「预览清单」需二次确认。
- **检查更新（check updates）**：对来源注册表里的每个 URL 做**纯手动**检查（浅克隆 → 逐文件比内容 → 三类清单：新增/覆盖/本地独有保留）。**永不自动写盘**、无任何定时任务。
- **来源记录（skill source record）**：`skill-sources.json`（schemaVersion 1）记录「哪个技能来自哪个 URL 的哪个 commit」；**不得**命名为 `skills.json`（已被退役的 ADR-0005 共享模块占用）。缺失/损坏时降级为空注册表，不阻塞主功能。
- **外部打开（open）**：前端只能传**闭集枚举**（`agents-md` / `context-md` / `skills-root`）或某个受管技能文件路径，路径由 Rust 从 `dshhome` helper 推导或按 ADR-0007 D10 校验归属 —— 前端**结构上无法**打开任意路径。退回链：配置的编辑器 → 系统默认程序（`ShellExecuteW`）→ 报错提示。
- **可配置编辑器**：`AppConfig.editor_command`（空 = 系统默认程序）。自由文本、引号感知切分，支持 `code --wait` 这类命令行。首次点击编辑弹一次性引导。

## MCP server 管理（ADR-0006）

- **MCP server 条目**：cordis 配置树中的一行，`name: '@deepseek-ai/dsh-mcp-client'`，其 `config.serverName` 是该服务器面向模型的命名空间。一个条目 = 一个 MCP server。仅桥接 Tools（resources / prompts 不支持）。
- **serverName**：条目的面向模型命名空间，工具名形如 `mcp__<serverName>__<tool>`。须匹配 `[A-Za-z0-9_-]{1,32}`，且在同一**注册作用域**内唯一。启动器以它作为管理标识键（而非行 id）。
- **行 id（rowId）**：条目的 cordis 标识；启动器声明的行固定用 `mcp-<serverName>`。
- **受管 MCP 区块（managed MCP block）**：启动器在 `$DSH_HOME/cordis.patch.yml`（机器级）中拥有的一段（marker 包裹），**两段式**——`insert:` 声明段 + `id`/`disabled:` 定向段。块外内容逐字节保留。
- **受管声明 / 外部声明**：条目的 `- insert:` 行位于受管区块内 = **受管声明**（`remove` 可真删除）；由 bundle、profile patch、home 文件用户区或 `--patch` 给出 = **外部声明**（`remove` 仅撤销定向覆盖，声明仍在）。
- **定向覆盖（id-targeted override）**：`- id: <rowId>` + `disabled:` 形式的独立 patch 条目，用于覆盖既有行的启停；可命中同层后置条目与**全部更早层**（home 层是最后持久层，故机器级定向覆盖对全层有效）。
- **注册作用域（registration scope）**：官方对 `serverName` 唯一性的强制范围；同一作用域内重复时，**后加载的实例在加载期失败，先前实例不受影响**。启动器只管理 root 作用域。
- **MCP 状态**：`missing`（合成树无该 serverName 的行）| `enabled`（有效 `disabled != true`）| `disabled`（有效 `disabled == true`）。行级另有无状态标记：`expression`（`disabled` 为 `!!js`，拒绝覆盖）| `conflict`（serverName 重复）| `dangerous`（`failOnStartupError: true`，可使 harness 启动中止）。
- **MCP 前置（prerequisite）**：`@deepseek-ai/dsh-mcp-client` 在受管 profile 的**可解析性**（非 `dependencies` 成员、非 bundle）；缺失时 MCP 页显示横幅并引导至插件页安装，**MCP 页不重复建设装卸入口**。
- **危险字段 `failOnStartupError`**：官方默认 `false`（失败仅 warn、harness 照常启动）；设为 `true` 时初始连接失败会让该 fiber FAILED，进而使**整个 harness 启动中止**。故 `disabled: true` 是该行唯一安全的隔离手段；启动器结构化通道不暴露该字段。
- **可观测性边界**：官方**不存在**列 MCP 状态或列工具的 CLI；`--dump-config` **不启动插件**、只证明**配置合成层**；`pluginInventory/list` 为 Remote-only 且不含 `serverName` 与 `mcp__*` 工具名。故脚本化证据只有「重新 dump 的目标行 `disabled`」与「dsh stderr 的 logger 行 + 有界窗口」，工具是否出现由人工确认（ADR-0006 D12 / §Testing 3.3）。

## 镜像源（镜像维度）

- **npm registry**：npm 通道装包 + pnpm 依赖安装的 registry。
- **GitHub 加速**：源码 clone（git 镜像）与 GitHub Release 下载（代理）共用。
- **Node/Git 二进制源**：工具链安装包的下载源。

## 生命周期

- **启动**：spawn `dsh web --port <p>`，注入工具链 PATH，CWD = 运行目录（dsh 官方默认）。
- **停止**：Windows 用 `taskkill /PID /T` 模拟优雅停止（1 秒等待，未退出即 `/F` 强杀），随后按端口兜底清剿（清剿前校验监听进程确为 dsh，防误杀）。**无官方 stop 命令**。
- **托盘**：关闭窗口→托盘；托盘菜单 = 打开主窗口 / 启动 / 停止 / 重启 / 退出。
- **设置开关（共 6 项）**：closeExits 关闭直接退出 / minimizeToTray 最小化到托盘 / keepDshOnExit 退出驻留 dsh / keepDshHomeOnUninstall 卸载保留 DSH_HOME（默认开）/ autoStartDsh 启动时自动启动 dsh / autoOpenBrowser 启动时自动打开 Web GUI。
- **退出**：托盘"退出"先优雅停止 dsh，再退出启动器；keepDshOnExit 开则驻留直接退出。
- **重启**：停止 → 用相同配置（端口）重新启动；端口被占则提示手动改。
- **运行状态检测**：事件驱动（Rust 持有 dsh 子进程句柄，退出即收状态）+ 端口探活（启动/停止/收养校验）+ 后端每 5 秒对账线程兜底（v0.4.13）。
- **就绪（ready）**：端口监听者归属为托管 pid 或命令行形如 dsh 的实例；**无关进程占端口不算就绪**（否则"停止"会误杀该进程）。
- **接管（take over）**：对外部实例的显式动作 —— 停止该实例并由启动器重新拉起，使 token 可被 stdout 捕获。会中断该实例上的会话，故**必须用户确认**。
