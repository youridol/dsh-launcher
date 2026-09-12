# dsh-launcher 设计总览

> 决策依据见 `CONTEXT.md`（词表）与 `docs/adr/`（ADR-0001~0009）。
>
> **本文档对齐说明**（ADR-0009 D14）：§3 的模块划分此前严重滞后（列出的 `core/install.rs`、
> `core/mirror.rs`、`core/backup.rs` 与顶层 `logging.rs`/`events.rs` **均不存在**，
> 且未收录 ADR-0005~0008 引入的 `plugin/`、`mcp/`、`skill/` 三棵子树）。
> 现按 `src-tauri/src/` 的实际结构重写，并逐项对照通过。

## 1. 产品定位

**dsh-launcher** 是 deepseek-harness（`dsh`）的 Windows 桌面启动器与管理器：

- **不改 dsh 源码**，只复用官方 CLI/接口（`dsh web`、`dsh plugin`、`dsh --dump-config`、npm 包、GitHub tag）。
- **全局单版本模型**：dsh 与工具链都是全局安装、单一版本；切换版本 = 先卸载再装（ADR-0003）。
- **职责收敛**：安装/卸载/更新、工具链检测与一键补齐、生命周期控制、日志全域、Web GUI 内嵌、
  插件管理（ADR-0005）、MCP server 管理（ADR-0006）、技能管理（ADR-0007/0008）。

## 2. 技术栈（最终裁定）

| 层 | 选型 | 职责 |
|---|---|---|
| 后端 | **Rust（Tauri 2）** | 进程生命周期、端口探测、工具链检测/安装、日志落盘、配置持久化、版本/通道管理、事件推送、插件/MCP/技能管理 |
| 前端 | **TypeScript + React 19 + shadcn/ui（base-nova 风格，UI 基元基于 `@base-ui/react`）+ Tailwind CSS 4** | UI 渲染、配置表单、版本列表、日志流、管理面板、WebView 内嵌 |
| 桌面 | **Tauri 2（WebView2）** | 窗口、托盘、系统集成；**抛弃 Electron**（Q28 裁定） |
| 分发 | **NSIS 安装包 + 便携版（zip）** | Windows 两种分发形态 |

## 3. 模块划分（Rust 后端，与 `src-tauri/src/` 实际结构一致）

```
src-tauri/src/
├── main.rs               # 二进制入口：判定 CLI 模式，否则进 GUI
├── lib.rs                # Tauri 应用装配：状态注册、托盘、事件、invoke_handler、panic hook
├── cli.rs                # 无 GUI CLI（plugin / skill / mcp），与 GUI 共用同一 core
├── commands/             # Tauri IPC 薄壳（只做 spawn_blocking + 事件广播）
│   ├── mod.rs
│   ├── dsh.rs            #   dsh 生命周期：get_status/get_web_url/probe_web_ready/
│   │                     #   start|stop|restart_dsh/create_web_gui_window/桌面快捷方式/窗口图标
│   ├── config.rs         #   配置读写：端口 / 镜像源 / 开关 / GitHub Token / 外部编辑器
│   ├── toolchain.rs      #   工具链：detect / install / uninstall / batch_*
│   ├── version.rs        #   版本与通道：list_versions / get_installed_version /
│   │                     #   install_version / get_install_paths / uninstall
│   ├── logs.rs           #   日志文件列表与读取（含路径穿越防护）
│   ├── plugin.rs         #   插件：list / install / set_state / uninstall / sync / repair
│   ├── skill.rs          #   技能：list / set_enabled / delete / import / update / sources / open
│   └── mcp.rs            #   MCP server：list / add / remove / set_state
└── core/                 # 核心逻辑（唯一允许触碰系统与 dsh 的层）
    ├── mod.rs
    ├── process.rs        #   spawn dsh、PID 跟踪、优雅停止/强杀、启动探活、状态对账、崩溃归因
    ├── port.rs           #   端口探测（TCP 探活 + HTTP 就绪探测）
    ├── command.rs        #   统一子进程创建（CREATE_NO_WINDOW / cmd 包装 / 带超时执行 / 杀进程树）
    ├── stream.rs         #   流式命令执行：逐行读 stdout/stderr → 日志 + 进度回调
    ├── pathutil.rs       #   PATH 注入与用户 PATH 持久化（HKCU\Environment）、Python 安装目录枚举
    ├── text.rs           #   子进程输出解码（UTF-8 优先 + 代码页回退，修中文乱码）
    ├── logging.rs        #   全域日志：落盘/轮转/保留、前端事件推送、web URL 缓存
    ├── events.rs         #   Tauri 事件名与进度载荷定义
    ├── config.rs         #   启动器配置持久化（%APPDATA%，含 DPAPI Token 加密与原子写）
    ├── profile.rs        #   **唯一**调用 dsh/pnpm 与读写 profile 文件的适配器
    ├── dshhome.rs        #   **唯一** DSH_HOME / agentsHome 解析实现 + 官方扫描根常量集中处
    ├── github.rs         #   GitHub 通道：tag 查询、clone/build、全局 shim、dsh 归属探测
    ├── toolchain.rs      #   工具链下载/解压/安装/卸载（Node/Git/Python）
    ├── tray.rs           #   系统托盘与窗口关闭/最小化行为
    ├── plugin/           #   插件管理（ADR-0005）
    │   ├── mod.rs        #     服务门面：状态机 + 受管区块 + 官方通道调用 + 幂等/回滚
    │   ├── state.rs      #     生命周期状态机（纯函数）
    │   ├── spec.rs       #     依赖 spec 分类与解析
    │   ├── registry.rs   #     插件注册表（%APPDATA%\plugins.json，缓存）
    │   ├── managed.rs    #     受管区块通用层（marker 家族 + 同文件写锁 + 原子写）
    │   ├── dump.rs       #     dsh --dump-config 行级解析 + MCP 行提取
    │   └── sync.rs       #     upstream 同步计划生成
    ├── mcp/              #   MCP server 管理（ADR-0006）
    │   ├── mod.rs        #     服务门面 + 写后校验与回滚
    │   ├── entry.rs      #     条目模型（声明/定向/区块）
    │   ├── block.rs      #     受管 MCP 区块两段式语法（解析 + 渲染）
    │   ├── state.rs      #     三态 / origin / 行级标记 / 非法转换校验（纯函数）
    │   ├── validate.rs   #     官方两条校验（serverName 形态 + 唯一性）
    │   └── prereq.rs     #     前置可解析性探测
    └── skill/            #   技能管理（ADR-0007/0008）
        ├── mod.rs
        ├── frontmatter.rs#     SKILL.md frontmatter 逐行外科手术（纯函数）
        ├── scan.rs       #     只读扫描 + rank 标注 + 受管根归属判定
        ├── manage.rs     #     启停/删除：身份校验 → 备份 → 原子写 → 复验 → 回滚
        ├── import.rs     #     从 URL 导入：递归扁平化 + 文件级覆盖
        ├── source.rs     #     来源注册表（skill-sources.json）
        ├── update.rs     #     手动检查更新（纯只读，永不自动写盘）
        ├── editor.rs     #     外部打开（闭集目标 + 可配置编辑器 + ShellExecuteW）
        └── sharing.rs    #     共享真源管理（前端入口已退役，保留供 CLI）
```

### 3.1 分层纪律（违反即视为架构缺陷）

| 约束 | 理由 | 落点 |
|---|---|---|
| `core/profile.rs` 是**唯一**调用 `dsh`/`pnpm` 与读写 profile 文件的适配器 | 状态机/幂等/回滚语义只有一处 | `core/profile.rs:1-8` |
| `core/dshhome.rs` 是**唯一** DSH_HOME / agentsHome 解析实现 | 避免各处拼路径导致"改错目录" | `core/dshhome.rs:1-19` |
| 写 profile 文件只有**一处**（失败回滚），且其后必跟官方 `dsh plugin install` 收敛 | 官方无"回退到任意历史 lock 状态"的能力（ADR-0006 D18） | `core/plugin/mod.rs:575-604` |
| `commands/*` 只做 `spawn_blocking` 与事件广播 | IPC 薄壳，业务全在 core | `commands/*.rs` 各文件头注释 |
| 前端不得传任意路径 | 结构上消除路径穿越（闭集枚举 + 受管根校验） | `core/skill/editor.rs:1-13` |

## 4. 核心流程

### 4.1 工具链检测与补齐
1. 检测清单：Node.js(22.19+/24+)、npm(随 Node)、pnpm、Git(2.26+)、Python(3.10+，可选)。
2. 状态：`present` / `missing` / `mismatch`（版本门槛按数值比较，见 `commands/toolchain.rs`）。
3. Node 走用户级 zip 解压（免管理员）并写入**用户 PATH**；Git/Python 走官方安装包（需 UAC）。
4. 网络问题由镜像源解决（npm registry / GitHub 加速 / Node 二进制独立配置）。

### 4.2 dsh 安装（双通道，ADR-0001/0003）
- **npm 通道**：`npm i -g @deepseek-ai/dsh@<version>`。
- **GitHub 通道**：`git clone --depth 1 --branch <tag>` → `pnpm install` → `pnpm build` → 建全局 `dsh.cmd` shim。
- **切换通道时先清理对侧**，保证全局只有一个通道的 dsh 生效。
- 启动器自建 shim 指向损坏目录时**静态短路**，绝不执行 `dsh`（否则 pnpm 递归进程爆炸）。

### 4.3 生命周期（ADR-0002）
- **启动**：按来源选择 `dsh web --port <p> --no-open`（npm 全局包）或直接 `node --import tsx/esm apps/cli/src/bin.ts web ...`（GitHub 通道）。
- **停止**：`taskkill /PID /T` 优雅停止 → 1s 未退即 `/F` 强杀 → 端口级兜底清剿（**清剿前校验监听进程确为 dsh**，防误杀）。
- **状态**：事件驱动（监视线程）+ 端口探活 + **5s 状态对账**（收养/收敛）。
- **启动就绪收敛（v0.9.1 修复）**：`Starting → Running` 由**两条路径**共同保证 ——
  启动探活线程（8s 内快速判定"启动即崩"并归因不兼容插件，ADR-0005 D12）与
  **5s 对账线程的 Starting 分支**（无上限持续探测，直到端口就绪或进程退出）。
  收敛判据 = 端口监听者归属为 `Managed`（本托管 pid）或 `Adopted`（命令行形如 dsh）；
  `Foreign`（无关进程占端口）绝不判定就绪。**禁止**任何"超时后放弃、状态永久停留
  Starting"的实现（曾导致前端「内嵌打开」误报"端口未监听"）。
- **收养**：启动时若配置端口已被 dsh 监听，恢复 Running（先做进程身份校验）。
  收养时缓存的访问地址**必须经 HTTP 探测确认对当前监听者仍有效**才沿用（dsh 的 token
  是**进程级随机数**，仅从该进程 stdout 打印）：失效即清除并交由前端询问是否**接管**。
- **托管 / 外部实例的区分（v0.9.1）**：`is_managed`（pid != 0）暴露给前端 —— 决定
  "继续等待 token"（托管，只是尚未打印）还是"询问接管"（外部实例，token 原理上不可得，
  需 `take_over_dsh` 停止后由启动器重新拉起以捕获 token）。

### 4.4 日志
- 采集：dsh stdout/stderr（落盘后 tail）与启动器自身日志，统一格式。
- 落盘：按天轮转 + 单文件 10MB 切割（级联保留 5 份），保留 30 天。
- 前端：`log://line` 事件实时流 + 文件视图 + 导出。
- Token：**日志正文保留明文**（产品决策，ADR-0009 D5）：用户需从日志面板复制完整带 token
  地址在外部浏览器打开（裸 URL 会被 dsh 401 拒绝）。日志仅本机用户可读（LOCALAPPDATA）。
  注：GitHub PAT 的口径不同 —— 它经 DPAPI 加密落盘且不写入日志。

### 4.5 窗口与托盘
- 无边框主窗口 + 自绘标题栏（最小化/最大化/关闭/日志开关），三栏布局（侧栏 | 主内容 | 日志）。
- 内嵌 WebView2 渲染 dsh Web UI（`create_web_gui_window`，含高清任务栏图标与回环链接放行）。
- 托盘菜单：打开主窗口 / 启动 / 停止 / 重启 / 退出。
- 设置开关（7 项）：关闭直接退出 / 最小化到托盘 / 退出驻留 dsh / 卸载保留 DSH_HOME /
  启动自动启动 dsh / 启动自动打开 Web GUI / 启动自动同步插件。

### 4.6 管理能力（ADR-0005~0008）
- **插件**：按包名启停（写 profile 受管区块，dsh 热重载）、装卸（官方通道 + 自动重启）、upstream 同步（git 钉 sha）。
- **MCP**：按 `serverName` 增删启停（写机器级受管区块，**不重启** dsh；`config` 不透明保真）。
- **技能**：列出/启停（改写 frontmatter `disable-model-invocation`）/可恢复删除/从 URL 导入/手动检查更新/外部编辑。

## 5. 数据与配置

| 用途 | 位置 |
|---|---|
| 启动器配置 | `%APPDATA%\dsh-launcher\config.json`（GitHub Token 经 DPAPI 加密落盘） |
| 插件注册表 | `%APPDATA%\dsh-launcher\plugins.json`（缓存，磁盘为事实源） |
| 技能来源注册表 | `%APPDATA%\dsh-launcher\skill-sources.json` |
| 技能共享偏好 | `%APPDATA%\dsh-launcher\skills.json` |
| 日志 | `%LOCALAPPDATA%\dsh-launcher\logs\<date>.log[.N]` |
| 备份 | `%LOCALAPPDATA%\dsh-launcher\backups\{plugins,mcp,skills}\...` |
| GitHub 通道安装目录 | `%LOCALAPPDATA%\dsh-launcher\github-dsh\deepseek-harness` |
| 工具链 | `%LOCALAPPDATA%\dsh-launcher\toolchain\{node,...}` |
| DSH_HOME | **只读展示、不接管**（`%USERPROFILE%\.dsh`，卸载时默认保留、可开关） |

## 6. 版本策略（ADR-0004）

- 启动器版本独立于 dsh，从 **0.1.0** 起，**0.0.1 递进、0.1.9→0.2.0 十进一**（非语义化版本）。
- **五处同步**：`package.json` + `package-lock.json`（顶层与 `packages[""]` 两处）+
  `src-tauri/Cargo.toml` + `src-tauri/tauri.conf.json` + `src-tauri/Cargo.lock`。
  版本变更的唯一入口是 `scripts/bump-version.mjs`；一致性由
  `scripts/check-version-sync.mjs`（CI 门禁）与 `tests/version_sync_test.rs` 双重守护。
- `CHANGELOG.md` 顶部插中文条目。

## 7. 分发

- **NSIS 安装包**（简体中文、免管理员、currentUser）+ **便携版 zip**。
- DoD：全流程冒烟（检测→装工具链→装 dsh→启动→内嵌 web UI 可交互→停止→卸载）在干净 Windows
  跑通；日志导出含 dsh+启动器全量；NSIS 与便携版均可安装/运行。

## 8. 质量门禁（ADR-0009）

| 门禁 | 命令 / 位置 |
|---|---|
| 版本一致性（五文件六落点） | `node scripts/check-version-sync.mjs`（CI + Release） |
| 前端类型检查 | `npx tsc --noEmit` 与 `npx tsc -p tsconfig.node.json --noEmit` |
| 前端布局回归 | `python scripts/verify-layout.py`（33 项断言，含移动端首屏不遮挡） |
| CSP 回归 | `python scripts/verify-csp.py`（真实产物 + 真实配置，断言零违规） |
| Rust 静态检查 | `cargo check --all-targets`（零警告） |
| Rust 测试 | `cargo test --lib` + 集成测试清单（见 `ci.yml`） |
| 网络/环境集成 | `.github/workflows/nightly.yml`（`--ignored`，每日，不阻断 master） |
| 端到端验收 | `scripts/e2e-adr0006.ps1`（真实 dsh + 隔离 DSH_HOME，参数化路径） |
