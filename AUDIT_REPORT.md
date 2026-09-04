# dsh-launcher 全链路代码审计报告（严格模式）

- 审计目标：`Y:\dsh-launcher`（Tauri 2 + React 19 + TS + Vite + shadcn/ui 的 Windows 桌面启动器）
- 审计基线：HEAD `1b96ca9`（v0.4.12），工作区干净（无未提交改动）
- 审计日期：2026-09（快照时间）
- 审计方法：逐文件通读 Rust 后端（`src-tauri/src/` 约 7k 行）+ 前端（`src/` 31 个文件，子代理通读）+ 配置/CI/脚本/文档（子代理通读）+ 交叉静态扫描 + 实测验证

---

## 摘要与总体结论

- 总体质量：**良好偏上**。架构分层清晰（core/commands/前端 IPC 封装），注释详尽、错误处理整体规范、
  测试覆盖了历次修复点；TS 严格模式编译干净、无 XSS 注入面、无 any；IPC 面（24 命令 + 4 事件）前后端完全一致。
- 未发现**严重**级问题；发现**高**级 4 项（功能失效/进程误杀/CI 盲区）、**中**级约 16 项、**低/提示** 40 余项。
- 必须优先处理的三件事：
  1. **2.2 端口误杀防护**——停止/清剿逻辑可能强杀占用同一端口的无关进程（含无身份校验的 adopt/port 清剿）；
  2. **2.1 + 2.3 假功能**——npm registry 镜像配置完全未接线；工具链 `mismatch` 状态永不产生、GitHub tag 排序非 semver；
  3. **凭据暴露面收口**——2.4（GitHub PAT 明文落盘/回传 IPC/进 git 命令行）+ 2.5（dsh web token 进明文日志与日志流），以及 2.7/2.8 子进程与网络操作全程无超时/取消。
- 文档/实现漂移是本仓库的显著共性问题（CONTEXT/ADR/DESIGN 多处声明与代码现状不符），建议审计后做一轮文档对账。

---

## 0. 实测验证基线（已在本机执行）

| 验证项 | 结果 |
|---|---|
| `npx tsc --noEmit` | ✅ 0 错误 |
| `npm run build`（tsc + vite build） | ✅ 成功（bundle 409.6 KB / gzip 128.2 KB） |
| `cargo check --manifest-path src-tauri/Cargo.toml --all-targets` | ✅ 0 错误；1 警告：`tests/direct_node_capture_test.rs:117` `unused_mut`（CI 未跑 `--all-targets`，不会被拦截，见 §5.1/T1） |
| `cargo test --lib` | ✅ 27/27 通过（0.01s） |
| 版本一致性（package.json / package-lock / Cargo.toml / Cargo.lock / tauri.conf.json / CHANGELOG 顶部） | ✅ 全部 0.4.12 |

> 注：CI 只跑 4 个集成测试（config_default / tray_icon / concurrency / icon_window），
> 其余 5 个测试文件（channel_mutex / direct_node_capture / github_tag / npm_versions / path_inject）
> 不在 CI 内，其中 `cargo check` 也未带 `--all-targets`，测试代码的编译错误可逃过 CI（见 §5.1）。

---

## 1. 严重级别定义

- **严重**：会造成数据丢失 / 非预期进程操作 / 直接可利用的安全或可靠性缺陷
- **高**：明确的功能失效（含"配置了但完全不生效"）、资源泄漏或稳定复现缺陷
- **中**：在特定条件下才触发的缺陷 / 竞态 / 边界问题 / 中等风险
- **低**：代码质量、重复、死代码、文档漂移、健壮性不足
- **提示**：风格 / 潜在改进

---

## 2. 后端（Rust）核心发现

### 2.1 【高】npm registry 镜像配置完全不生效（死功能）

- 位置：`src-tauri/src/commands/config.rs:76-108`（保存）、`src-tauri/src/core/config.rs:17`（字段）、`src-tauri/src/commands/version.rs:294-315, 366-397`、`src-tauri/src/core/github.rs:260-296`
- 问题：`npm_registry` 只被「保存」「回读给前端」，在**任何**实际执行路径上都未被使用：`npm install -g @deepseek-ai/dsh@<v>`、`npm view`、`pnpm install`（GitHub 通道构建）、`npm i -g pnpm` 均未传 `--registry` 或设置 `npm_config_registry`。
- 影响：UI 上配置 "npm registry 镜像" 对 npm 通道安装、pnpm 依赖安装完全不生效（GitHub 通道 pnpm 也走用户默认 registry）。文档（CONTEXT.md/DESIGN.md §4.1）宣称该镜像可用 → 功能与文档双重失效。
- 建议：① 在 `install_npm_version` / `install_pnpm` / GitHub 通道 `pnpm install`（`github.rs`）处，依据配置追加 `--registry <npm_registry>`（或对子进程设 `npm_config_registry` 环境变量）；② 或从 UI 移除该输入项。

### 2.2 【高】"已收养/外部 dsh"停止路径可能强杀无关进程

- 位置：`src-tauri/src/lib.rs:178-182`（启动时把"配置端口被占用"一律视为 dsh 在运行 → `adopt_running`）；`src-tauri/src/core/process.rs:89-104`（adopt）、`323-341`（按端口查 PID）、`385-427`（停止后端口清剿，**无条件强杀端口上任何监听者**）
- 问题：没有任何「该端口监听者确实是 dsh」的进程身份校验（ADR-0002 要求 process match「命令行含 dsh web」判据，未实现）。若用户配置端口恰好被任意其他程序（数据库/代理/另一服务）占用：
  1. 启动器启动即把状态置为 Running（`adopt_running`）；
  2. 用户点"停止/重启"→ `pid_by_port` → `taskkill /PID <pid> /T`，若 1s 内未退再 `/F`，随后 5 轮端口清剿会**强杀该端口上任何残留监听进程**；
  3. 代码注释声称"避免误杀刚重启的新实例"，但实际**没有做任何 PID 身份比对**（`process.rs:399-400` 处 `let _ = round;` 是未实现的占位残留）。
- 影响：可能终止与 dsh 无关的第三方进程（属高危误操作面）。
- 建议：进程身份校验后再动刀：查监听进程命令行/可执行名（PowerShell `Get-CimInstance Win32_Process` 过滤含 `dsh`/`bin.ts`/`deepseek-harness` 特征且确实是本启动器 spawn 链）后再 taskkill；对外部监听者仅提示、不强杀；清剿循环删除占位变量并实现真实比对。

### 2.3 【高】npm/GitHub 通道"版本列表排序"与"工具链版本门槛"为假实现

- 排序（`src-tauri/src/core/github.rs:168-173`）：`tags.sort_by(|a,b| b.cmp(a))` 是**字典序**而非 semver 序。
  - 现在 tag 为 `dsh-v0.1.x`，宽度一致暂时无感；一旦出现 `dsh-v0.1.1-rc.10` vs `rc.2`，或 `v0.10.0` vs `v0.9.x`，字典序将把更新版本排到后面（rc.2 > rc.10、0.9 > 0.10）。"最新版本置顶/默认安装最新"会选错版本。
  - 建议：用 semver 比较（引入 `semver` crate 或实现数值段比较）。
- 工具链版本门槛（`src-tauri/src/commands/toolchain.rs:319-402`、DESIGN.md §4.1）：声明 required `22.19+ / 24+`、`Git 2.26+`、`Python 3.10+`，`state` 只产生 `present|missing`，**全仓库无任何版本号比较**——CONTEXT/DESIGN 定义的 `mismatch` 状态永不出现；Node 20 / Git 2.2x 一律显示绿色 present。前端 `ToolchainItem.state` 类型也不含 mismatch。
  - 建议：加版本解析与门槛比较产出 mismatch；或把 required 字段改回纯展示并更新文档。

### 2.4 【中】GitHub PAT：明文落盘 + 明文回传 IPC + 出现在 git 进程命令行

- 位置：`src-tauri/src/core/config.rs:21,76-83`（明文存 `%APPDATA%\dsh-launcher\config.json`）；`src-tauri/src/commands/config.rs:16,46-48`（`get_config` 把 token 原样返回前端）；`src-tauri/src/core/github.rs:66-78`（`git -c http.extraheader=AUTHORIZATION: bearer <token>` 把 PAT 放进**子进程命令行参数**）
- 影响：PAT 以明文落盘；同用户进程/取证可读子进程命令行；前端渲染层持有完整 PAT（结合 CSP 为 null、任一 XSS 即可外带）。日志未打印 token 是做得好的，但上述三处仍暴露。
- 建议：① 落盘用 DPAPI（`CryptProtectData`，windows crate 已依赖）或 Credential Manager；② `get_config` 脱敏返回（只回传是否已设置）；③ git 认证改环境变量方式（`GIT_CONFIG_COUNT=1/GIT_CONFIG_KEY_0=http.extraheader/GIT_CONFIG_VALUE_0=...`）或 `GIT_ASKPASS`，避免进命令行。

### 2.5 【中】dsh web 访问 token 进入明文日志与前端日志流

- 位置：`src-tauri/src/core/process.rs:726-735`（tail stdout 每行落日志，含 `?token=…` 的 URL 行被 `logger.log` 原样写入 `<date>.log` 并推送到前端 `log://line` 事件）；`src-tauri/src/core/logging.rs:324-355`（从日志文件全文反向提取含 token URL）
- 影响：带凭据 URL 以明文保留在 30 天轮转日志中，并展示在前端日志面板（`LogPanel` 流式展示/文件视图，文件视图还支持把整份日志导出）。凡日志泄漏（崩溃转储、用户截图分享、文件同步）即泄漏 dsh web 会话凭据。
- 建议：写日志/推送前对 `token=` 值打码（如 `token=***`），仅内存 `web_url` 保留原始值；从日志恢复 URL 的兜底逻辑相应改为存"恢复所需最小信息"或对恢复值单独加密。

### 2.6 【中】`AppConfig` 读-改-写无锁 → 配置丢失更新竞态

- 位置：`src-tauri/src/commands/config.rs:52-60/66-74/78-108/112-132`（每个 set 命令 `load() → 改一字段 → save() 全量写回`）
- 问题：命令均为 async + `spawn_blocking`，可并发执行。并发保存"端口"与"镜像源"（或与 auto_start 线程的 `load`）时，后写者覆盖先写者未修改字段 → 配置丢更新。
- 建议：配置模块加 `Mutex<()>`（或 `Arc<Mutex<AppConfig>>` 托管到 `AppState`），set_* 统一持锁 RMW；`save` 改原子写（临时文件 + rename）。

### 2.7 【中】`run_streamed` 读线程 join 无超时；子进程孙进程持管道可致永久挂起

- 位置：`src-tauri/src/core/stream.rs:44-97`（`child.wait()` 后 `t_out/t_err.join()`，无超时）
- 问题：stdout/stderr 句柄被子进程树继承时，`cmd /C npm …` 退出但孙进程（如 npm 的后台脚本/daemon、git credential helper）仍持有管道写端 → 读线程 `read()` 永不返回 → `join()` 永久阻塞 → 安装/构建命令永不返回，前端进度卡死（无任何超时护栏）。`github.rs` clone/install/build 与 `version.rs` npm install 全部经此路径。
- 建议：join 加超时（超时后 drop 读线程句柄/管道即可令 read 返回）；或 wait 前先 drop 父进程持有的写端引用。同时为所有 `Command::output()`（npm view、git ls-remote、下载等）加进程级超时与超时 kill（见 2.8）。

### 2.8 【中】所有网络/子进程操作无超时 → 网络卡死时 UI 无限等待

- 位置：`src-tauri/src/core/toolchain.rs:120-133`（Invoke-WebRequest 无 `-TimeoutSec`）、`fetch_content_length` 同；`commands/version.rs:294-315`（npm view）、`core/github.rs:113-147`（git ls-remote）等全部 `.output()` 阻塞等待无超时。
- 影响：DNS 黑洞/镜像不可达/下载中断时无任何恢复路径；安装按钮进度停在 0%，用户只能杀进程。
- 建议：统一给命令执行加最长等待（如 60–120s）超时与超时 kill；PowerShell 下载加 `-TimeoutSec`（分块），并提供"取消"能力。

### 2.9 【中】配置/日志目录兜底到 "." 的行为

- 位置：`src-tauri/src/core/config.rs:86-91`、`logging.rs:314-322`、`github.rs:81-87`、`toolchain.rs:19-30`
- 问题：`APPDATA`/`LOCALAPPDATA` 缺失时把数据写进当前工作目录 "."，可能与便携版所在目录混淆（只读目录、UAC 目录等导致静默失败或污染 CWD）。`Logger::init()` 失败在 `lib.rs:141` 直接 `expect` panic（release 无控制台 → 用户无感知崩溃）。
- 建议：环境变量缺失时返回明确错误并 UI 提示，而非 "."；`Logger::init` 失败不要 panic，改为可恢复错误路径（或写入系统临时目录）。

### 2.10 【中】托盘/菜单构建失败 = 启动 panic

- 位置：`src-tauri/src/core/tray.rs:28-47,146`（7 处 `.expect()`）
- 影响：托盘菜单/图标构建在异常平台/环境失败会让整个应用启动即 panic（release 无控制台、静默退出）。
- 建议：`setup_tray` 返回 `Result`，失败仅降级（无托盘运行）并落日志。

### 2.11 【中】状态机只靠事件 + 前端轮询，收养的外部 dsh 状态无法收敛

- 位置：`src-tauri/src/lib.rs:178-182` + `process.rs:89-104` + `commands/dsh.rs:116-120`（get_status 只读内存状态）
- 问题：CONTEXT/DESIGN 声称"端口探活每 5 秒兑底轮询"由 **Rust 兜底**，但后端并无周期探活；前端 StatusCard 5s 轮询仅读取内存状态。收养场景（launcher 启动时探测到外部 dsh）下若外部 dsh 之后被外部终止，后端没有探活线程把它收敛到 Stopped → UI 永远显示"运行中"，且期间 start 被拒（"正在运行或切换中"）、restart/stop 会走 2.2 的危险路径。
- **与文档矛盾**：ADR-0002 明确运行状态检测 = port probe + process match（命令行含 `dsh web`/`@deepseek-ai/dsh`）、"端口探测是主判据"，且 DESIGN/CONTEXT 声明 Rust 5s 兜底探活——实现均未落实（无命令行匹配、无后端周期 reconcile），仅前端轮询读内存状态。
- 建议：后端加周期 reconcile（如每 5s：若 pid==0 且状态 Running → 端口探活，无监听则置 Stopped 并广播）；或 get_status 在 pid==0/Running 时做端口探活后再返回。

### 2.12 【低】Python 安装目录"版本降序"排序非版本感知

- 位置：`src-tauri/src/core/pathutil.rs:404-406`
- 问题：`dirs.sort_by(|a,b| b.cmp(a))` 对路径字符串排序：`Python39` > `Python310`（'9' > '1'）→ 多版本共存时探测优先选中 **Python 3.9** 而非 3.13，与注释"最新版本优先"矛盾（影响 detect 与安装确认）。
- 建议：从目录名解析 `3.x.y` 数值段排序。

### 2.13 【低】死代码 / 重复实现 / 残留占位

- `core/github.rs:346-361` `pub fn global_shim_is_ours()`：全仓库（src + tests）无调用者（已被 `probe_dsh_command` 取代）→ 死代码。
- `core/text.rs:65-70` `pub fn decode_line()`：无调用者（等价于 `decode`）→ 死代码。
- URL 提取逻辑重复两份：`core/process.rs:813-826` `extract_web_url` 与 `core/logging.rs:337-355` `find_web_url_in_content`（语义几乎一致，仅输入粒度不同）→ 建议合并为一份。
- Node PATH 注入双实现：`core/pathutil.rs:59-66`（`inject_node_path_into`，`command.rs:52` 已自动调用）与 `core/process.rs:244-267` 内联版（`where node` + 前缀拼接）→ 后者可直接复用前者。
- `commands/toolchain.rs:287-299`：回退分支内联重建 `cmd /C` + `CREATE_NO_WINDOW`（魔法数 `0x0800_0000` 第三次出现），应复用 `command::hidden_cmd(&fallback)`。
- git.exe 候选路径清单重复：`core/github.rs:53-57` 与 `commands/toolchain.rs:364-368`。
- `core/process.rs:399-400` `let _ = round;`：未完成逻辑占位（见 2.2）。
- "npm/pnpm 全局 prefix 回写到 node_dir" 仅存在于 `core/toolchain.rs:392-393` 注释，无对应实现（若依赖此行为需补实现或删注释）。
- `git` 卸载路径注释行 112 "镜像 API 或 git 均可 / curl 兜底" 与实际实现（只 git，失败即 Err）不符 → 注释漂移。
- 引用不存在的 `AGENTS.md`：`commands/dsh.rs:3`、`release.yml:214` 注释（仓库无该文件，属历史遗留）。

### 2.14 【低】健壮性杂项

- `core/process.rs` 全部共享锁用 `.unwrap()`（90/93/97/284-291/313/323/325/339/388/449-452/484/489/519/524/613/652-663）：任一持锁线程 panic 导致锁中毒后，后续所有命令 panic；建议统一 `unwrap_or_else(|e| e.into_inner())` 或 `parking_lot`。
- `lib.rs:87`：等 token ≤10s 拿不到时仍以裸 URL 开窗（与 401 修复注释相悖）；建议拿不到 token 时只聚焦主窗口并提示，不开可能 401 的窗口（与第 ① 段行为一致）。
- 日志清理只在 `Logger::init` 执行一次（`core/logging.rs:100`）：应用常驻超过 30 天不重启则不清理过期日志 → 建议按天定时清理（log 触发时顺带检查即可）。
- `commands/logs.rs:27-84`：`list_logs` 会把 `dsh-web-stdout.log / dsh-web-stderr.log`（dsh 每次启动被截断重建）混入日志文件列表；轮转档 `.log.1` 因扩展名非 `.log` 反而不在列表 → 文件视图与磁盘内容不一致。
- `core/stream.rs:111`：`read()` 错误被当作 EOF（`unwrap_or(0)`）静默截断输出 → 建议至少记一次日志。
- `core/process.rs:671-678`：stdout/stderr 落盘文件路径固定、tail 线程跨重启边界可能与新一轮 monitor 并存 150ms 造成少量重复/缺失日志行（低概率）。
- `lib.rs:196-219` 自动启动线程 1.5s 后再 `load()` 配置（与 setup 时判定的 `cfg.port` 可能不同步）。
- 版本锁定集中但分散在代码（`NODE_VERSION v22.19.0`、`GIT_VERSION 2.47.1`、Python `3.13.9`）：建议收敛为常量表/配置文件并配过期提示，避免随 dsh 要求升级而漏改。

---

## 3. 安全专项

| # | 级别 | 项 | 位置 | 说明与建议 |
|---|---|---|---|---|
| S1 | 中 | CSP 为 null | `tauri.conf.json:24-26` | 主窗口加载本地包，整体风险可控；但一旦引入远程内容或依赖投毒，CSP null + 宽权限将放大 XSS 后果。建议补最小 CSP（`default-src 'self'`、`style-src 'self' 'unsafe-inline'` 视 Tailwind 内联而定），并在发布前用 `npm audit` / 供应链扫描。 |
| S2 | 低 | capabilities 偏宽 | `capabilities/default.json` | `core:webview:allow-create-webview-window` + 全套 window 权限必要；`allow-set-webview-position/size`、`allow-destroy` 等若前端未使用可收紧（最小权限）。注意：**远程 dsh GUI 未列入任何 capability 的 remote 域 → 无法调用 IPC，这是正确隔离**，请勿放开。 |
| S3 | 中 | 未校验下载物完整性 | `core/toolchain.rs`、`github.rs` | Node zip / Git exe / Python exe / git clone 均无 checksum/签名校验（依赖 HTTPS）。建议固定 sha256（Node 官方提供 SHASUMS256.txt）校验后再解压执行。 |
| S4 | 低 | cmd.exe 包装透传远端可控字符串 | `core/command.rs:45-54` 用法（`version.rs:366-397`、`github.rs:260-296`） | 经 `cmd /C` 执行的参数若含 `&|<>^` 会被 cmd 二次解释。npm 通道版本串来自 registry（`npm view` 输出）经 `@deepseek-ai/dsh@<version>` 拼接——恶意/被镜像投毒的 registry 可返回含元字符版本号 → 本地命令注入。建议安装前用白名单正则校验版本串 `^[0-9A-Za-z][0-9A-Za-z.+-]*$`。GitHub 通道 tag 走 git.exe 直接参数（无 cmd），风险低。 |
| S5 | 低 | PowerShell 单引号转义模式 | 全局 | 均为 `'…'` + `''` 双写转义，模式正确；镜像/路径入 PS 字符串安全。 |
| S6 | 低 | 卸载 dsh 时删除 `~/.dsh` | `commands/version.rs:252-284` | 特征确认（profiles/settings.yaml）已降低误删风险；建议删除前额外要求用户输入确认词或二次弹窗，并记录删除时间/目录于日志。 |
| S7 | 提示 | 远端 GUI 凭据模型 | — | dsh web token 即会话凭据：见 2.5 打码建议；另建议在打开外部浏览器前提示（带 token URL 交予默认浏览器属凭据外发，当前 4.5/设计如此，仅提示）。 |

---

## 4. 前端（TypeScript/React）审计

> 由前端专项子代理逐文件通读 `src/` 全部 31 个文件后输出，主代理已复核关键证据
> （`reclampWidth` 无调用方、`versionSortDesc` localeCompare 平局、`onClose` 无传值等 grep 一致）。
> 总体结论：**未发现严重/高危项**；无 `any`/`@ts-ignore`/`dangerouslySetInnerHTML`，事件/定时器基本成对清理；
> 24 个 invoke 与 4 类事件名、载荷与 Rust 端完全一致。主要风险在布局联动与日志流渲染两处。

### 4.1 【中】M1 — 面板宽度与对侧面板状态联动缺失，主内容可被压破设计最小值
- 位置：`src/components/AppShell.tsx:96-121,124-138,245-250`；`src/hooks/useResizablePanel.ts:225-227,312`
- 问题：面板最大可用宽度依赖对侧 open 状态/宽度，但 clamp 只在「自身拖拽」「窗口 resize」发生；hook 已返回 `reclampWidth`（`useResizablePanel.ts:312`）却**全仓库无调用方**。复现：1024px 窗口，收起侧栏→右栏拖到 ~604px→展开侧栏 → 主内容仅 ~220px < 420 设计下限，内容溢出/挤压。
- 根因：宽度状态与 open 状态解耦但缺"对侧变化→本侧重算"闭环。
- 建议：① AppShell 加两个 effect：`[rightOpen]`→`sidebarResizer.reclampWidth()`、`[sidebarOpen]`→`rightResizer.reclampWidth()`；② 给 hook 加 `onCommitted` 回调让 AppShell 联动对侧；③ 视口断点穿越已有 resize 覆盖。

### 4.2 【中】M2 — LogPanel 实时日志流 O(n²) 退化
- 位置：`src/components/LogPanel.tsx:60-69,188-190,257-261`
- 问题：每条 `log://line`：整体拷贝 2000 行数组 + 全量 `stripEmoji` 正则重扫 + 巨型 `<pre>` 文本重设；`streamText` 未 memo。构建/安装高频日志下（本应用主场景）累计数千万字符级处理 → UI 卡顿。
- 建议：增量维护已格式化串（每行只 append 一条）；或最低限度 `useMemo(…,[streamLines])`；同帧多条日志可合并一次 setState。

### 4.3 【低】L1–L9
| 编号 | 位置 | 问题 / 建议 |
|---|---|---|
| L1 | `VersionPanel.tsx:192` | 2.5s 清进度 `setTimeout` 不登记清理；建议存 ref、卸载清理，并带安装序号令牌。 |
| L2 | `StatusCard.tsx:218-230,257-407` | 取消弹窗后内层 `waitForWebUrl` 仍轮询最多 40s/80 次 IPC（`alive()` 只在外层）；把取消令牌下渗到最内层循环。 |
| L3 | `StatusCard.tsx:128-145`、`ToolchainPanel.tsx:81-116`、`VersionPanel.tsx:116-174`、`LogPanel.tsx:56-78`、`WindowControls.tsx:14-32` | 5 处雷同 async-listen 模式：cleanup 先于 listen resolve 时监听器永不注销；StrictMode dev 下日志行翻倍/事件双跑。建议抽公共 `useTauriListener`。 |
| L4 | `LogPanel.tsx:117-139` | 挂载 effect 依赖依赖 `selected` 的 `refreshFiles` → 每次切文件都重建 30s 定时器 + 重复 IPC；backfill 与轮询拆成两个 effect。 |
| L5 | `useResizablePanel.ts:177-195` | 每 pointermove 直写 CSS 变量无 rAF 合并；高速拖拽布局抖动。拖动期宽度存 ref + rAF 写样式。 |
| L6 | `LogPanel.tsx:111-115` | 每条新日志强制 `scrollTop=0` 与用户滚动打架；改为仅在"贴顶"时自动回顶。 |
| L7 | `LogPanel.tsx:36-42,223-227`（对照 `AppShell.tsx:259`） | `onClose` prop 无调用方传值 = 死代码；删除 prop/分支与过时注释。 |
| L8 | `StatusCard.tsx:120-124,132-136,199-201` | 安装状态刷新逻辑重复 3 处；抽 `refreshInstalled`。 |
| L9 | `LogPanel.tsx:151-169` | 导出日志立即 `revokeObjectURL`（可能使下载失败）；文件名 `replace("/", "-")` 只换首个 `/`。延后 revoke、`replace(/[\\/]/g, "-")`。 |

### 4.4 【提示】I1–I13
- I1–I5：ui 组件死导出：`scroll-area.tsx:32-56` ScrollBar、`card.tsx:59-70,82-93` CardFooter/CardAction、`dialog.tsx:12-22` DialogTrigger/DialogClose（含 Overlay/Portal 导出多余）、`progress.tsx:52-80` ProgressLabel/ProgressValue、`badge.tsx:52`/`button.tsx:58` variants 无外部消费者。
- I6：`src/index.css:293-307` `.right-panel-container … pre` 规则连续重复两块（第一块被完全覆盖=死代码）；`scroll-area-viewport` 选择器拆两处（289-291/309-311）。
- I7：`src/index.css:9-43` `:root` 亮色变量永不生效（App.tsx 永久 `.dark`）= 死主题。
- I8：`src/lib/version.ts:25-33` 平局用 `b.localeCompare(a)` 非数字感知（审计员复核：例 rc.2/rc.10 实际因字典序巧合落对，但 **rc.9 vs rc.10 必错**——rc.9 会排到 rc.10 之前；预发布段应数值化比较）。
- I9：`src/lib/panel-layout.ts:32-34` `getDefaultRightPanelWidth(_viewportWidth)` 忽略入参返回常量，且 AppShell 同表达式重复 3 次。
- I10：`sonner.tsx/switch.tsx/separator.tsx:1` `"use client"` 在 Vite 下无意义；`sonner.tsx:35` 未 import React 却用 `React.CSSProperties`（UMD 命名空间依赖）。
- I11：`SettingsPanel.tsx:109-119` toggleSwitch 用渲染闭包快照构造 next；建议函数式更新。
- I12：`main.tsx:6`/`AppShell.tsx:142` 的 `as HTMLElement` 掩盖 null；`StatusCard.tsx:384-387` payload 未类型化。
- I13：`StatusCard.tsx:213,374-395` token URL 进外部浏览器历史 / 常驻渲染进程内存；建议 UI 掩码展示 token、外部打开前提示历史风险。

### 4.5 前端亮点
- IPC 封装 24 命令/4 事件与 Rust 完全一致；`port`/`channel`/`name`/`read_log` 前后端双重校验；无 XSS 注入面；
- StatusCard `openSeq` 流程序列令牌、ToolchainPanel/VersionPanel 的订阅守卫设计扎实。

---

## 5. 构建 / 配置 / CI / 脚本 / 文档审计（主代理 + 配置/CI 子代理合并去重）

> 严重级别分布（本域）：严重 0 · 高 0 · 中 6 · 低 9 · 提示 8（子代理原始口径）；
> 下文按合并去重后的编号列出，与后端/前端章节重复者给出交叉引用不再赘述。

### 5.1 CI 测试覆盖与类型检查盲区
- **T1【高】CI 无法拦截测试代码编译错误/警告**（同 §0 注）：`ci.yml` 的 `cargo check` 未带 `--all-targets`，且只运行 4/9 个集成测试（config_default / tray_icon / concurrency / icon_window）。`channel_mutex_logic_test`、`path_inject_test` 离线可跑却未进 CI；`direct_node_capture_test` 也未跑；`direct_node_capture_test.rs:117` 的 `unused_mut` 警告因此逃过 CI。建议：`cargo check --all-targets`（或 `cargo test --no-run`）+ 纳入 offline 测试；网络型测试用 `#[ignore]` + 显式运行。
- **T2【中】`vite.config.ts` 从不被类型检查且存在 2 个潜在类型错误**：`tsconfig.json` 仅 `include:["src"]`；实测 `npx tsc -p tsconfig.node.json --noEmit` 报 `vite.config.ts(4,18)` 缺 `node:path`、`(14,25)` 缺 `__dirname`（devDependencies 无 `@types/node`；vite 以 esbuild 转译配置故构建仍过）。建议：加 `@types/node`，CI 增加 `tsconfig.node.json` 的 `--noEmit` 检查（TS 5.8 实测可与 composite 共存）。
- **T3【低】** 无 lint 环节：无 ESLint / `cargo clippy` / `cargo fmt --check`；风格只靠人工统一。
- **T4【提示】** `tsconfig.node.json` 未开 `strict`/`noEmit` 等（与 T2 一并处理）。

### 5.2 Release 工作流竞态与语义缺陷（子代理新发现）
- **T5【中】tag 与 CHANGELOG/版本提交错位 1 个提交**（本地证据确凿）：tag `v0.4.5` 打在 e92b8bc（feature 提交，其 CHANGELOG 顶部为 [0.4.3]），[0.4.5] 条目位于下一提交 9b3170c；即 tag 建在 bump 提交推送**之前**（release.yml:181-201 tauri-action 在推送前于检出 SHA 上建 tag）。建议：bump 提交推送后再显式打 tag（或用 tauri-action 已存在 tag），使 tag 指向含对应 CHANGELOG 条目的提交。远端 tag 状态已实查存在（v0.4.5–v0.4.12），本地 clone 未 fetch 所致，无重复发布风险（原 §5.1 猜测解除）。
- **T6【中】Release 并发竞态**：`concurrency.group=release-${{ github.ref }}` 且 `cancel-in-progress:false` 仅排队；第二个排队 run 检出旧 SHA 后，第一个 run 已建 tag vX → 后 run `git push ... || echo`（release.yml:184）吞掉失败 → tauri-action 撞已存在 tag。建议：bump 前 `git fetch` 远端并重算"是否已发布"、push 失败不得吞错、或改用 cancel-in-progress。
- **T7【中】workflow_dispatch `version-bump` + `skip-release` 语义漏洞**：`skip-release=true` 仍执行"当前版本已发布→自动 patch 递增"并推送 `[skip ci]` 提交与 CHANGELOG 条目但不打 tag → 可堆出多条无 Release 的版本跳跃；组合语义落空。建议：skip-release 时跳过自动 bump 分支，并对两者做互斥校验。

### 5.3 生命周期/文档一致性（与后端章节交叉）
- **T8【中】** "Rust 5s 端口兜底探活"未实现：`core/port.rs:5` 注释、CONTEXT.md:40、ADR-0002:12 与实现不符 → 见 §2.11（含建议）。
- **T9【中】** 工具链 `mismatch` 状态从不产生：CONTEXT.md:19、commands/toolchain.rs:4 声明三态，detect_* 只判 present/missing，无版本阈值比对 → 见 §2.3（含建议）。
- **T10【低】停止时序文档 vs 实现**：ADR-0002/CONTEXT/DESIGN 称"优雅排空 ≤5s、二次信号"，实现为 `taskkill /T` 等 1s 即 `/F`（process.rs:356-383，v0.3.5 起加速）且无严格 SIGTERM 语义——建议更新文档或按需恢复 5s 窗口。
- **T11【低】文档事实过期/矛盾汇总**：
  - 开关数量：CONTEXT 称 3 个、DESIGN 称 4 个、实际 6 个（+autoStartDsh / autoOpenBrowser）；
  - 版本同步处数量：ADR-0004 三处、README:36 与 bump-version.mjs:2 注释四处、DESIGN:88 五处（含 Cargo.lock）——核对实现为**五处同步 + release.yml 提交六文件含 CHANGELOG，清单无遗漏**；
  - ADR-0004 "十进制 0.1.9→0.2.0" 语义未被 bump-version.mjs 执行（实际为 semver patch/minor/major，产出 0.4.9→0.4.12）；
  - ADR-0001 与 version.rs:4 的 npm "最新 0.1.1-rc.2" 版本事实过期；
  - CONTEXT.md/ADR-0003 "工具链系统全局、无用户级目录" 与实现矛盾（Node=LOCALAPPDATA 用户级 + HKCU PATH、Python per-user，仅 Git runas 系统级）；DESIGN §3 模块表内部亦有矛盾；
  - `AGENTS.md` 悬空引用：commands/dsh.rs:3、docs/adr/0004:5,10、release.yml:214（文件不存在）；
  - DESIGN §4.1 `mismatch`、§4.3 "5s 兜底"、github.rs curl/API 兜底注释均与实现不符（见 §2.3/§2.11/§2.13）。

### 5.4 依赖与打包
- **T12【低】** `package.json` 依赖归类：`shadcn`（CLI）、`tailwindcss`、`@tailwindcss/vite`、`tw-animate-css`、`@fontsource-variable/geist` 属构建期/打包期依赖，建议移入 devDependencies（`shadcn` 经 `index.css:3` import 引用，非无用依赖）。
- **T13【提示】** `package.json:31` `@types/react ^19.2.18` 与 lock 根 `^19.1.8` 区间不一致（lock 实际解析 19.2.18，`npm ci` 可过；lock 需重跑 install 刷新）。
- **T14【提示】** `Cargo.toml:28` windows 主依赖开启 `Win32_Graphics_Gdi` 但 src 无 GDI 调用（仅测试用；dev-dependency 已含同 feature）——可从主依赖裁剪。
- **T15【提示】** `release.yml:76-99` 用 pwsh `Out-File -Append` 写 GITHUB_OUTPUT：pwsh7 默认 UTF8 可工作，但 PS5.1 会写成 UTF-16 破坏解析——建议 `>> $env:GITHUB_OUTPUT`。
- **T16【提示】** release-notes awk（release.yml:152-159）：正文含 `## ` 行会截断、CRLF 带 `\r`（概率极低）。
- **T17【低】** 便携包命名前后不一致：`release.yml:226` `_x64_portable.zip`（下划线）vs 本地 dist-bundles 0.4.x `_x64-portable.zip`（连字符）。本地 dist-bundles 为 gitignored 产物（缺 0.4.3/0.4.4/0.4.5/0.4.7 仅为本地未同步），非仓库缺陷；建议统一命名避免跨版本混乱。
- **T18【低】** `scripts/verify-layout.py` 已全面失效（运行必 FAIL）：期望 Sidebar 260 / Right 640（:33-34）、拖拽 260→320/640→560、localStorage 键无 `-v3`（:100-101）；实现默认 340/340（panel-layout.ts:11-18）、键为 `*-v3`（AppShell.tsx:104,119）。建议重写或删除；且依赖 playwright（未声明 devDependencies）。
- **T19【低】** `scripts/generate-icons.ps1` 良好；`public/favicon.png` 不在生成清单（改图标后过期）；`icon.png`（运行时嵌入 512）与 `ico.png`（生成源 512）双源并存易分叉。

### 5.5 配置面提示
- **T20【低】** `tauri.conf.json:24-26` CSP=null → 见 §3 S1。
- **T21【提示】** `capabilities/default.json` 含前端未用的授权项（allow-create/destroy/show/set-focus/set-size/set-title/unmaximize、webview show/set-position/set-size 等）——回归确认后收敛（见 §3 S2）。
- **T22【提示】** `index.html:2` lang="en"（UI 全中文）；`components.json:4` style "base-nova" 非标准值；README:50-51 许可章节 bullet 混行。
- **T23【核对通过】** 五处版本号 + CHANGELOG 顶部 + 远端 tag v0.4.12 全部一致；Cargo.lock 提交与同步正确；防循环（[skip ci]）完备；npm/Rust 依赖均有实际使用；`windows:["main"]` 使远端内嵌窗口无 IPC 权限（隔离合理）。CI 中 tsc 与 `npm run build` 内 tsc 重复执行一次（冗余但无害）。
- **T24【提示】`bump-version.mjs` 澄清与健壮性**：该脚本仅 `import node:fs`、无任何子进程调用（"child_process 字符串拼接注入"预设不成立）；五文件顺序写非事务，中途失败会留下不一致状态（由一致性 guard 拦截、需手工对齐），且依赖 cwd=仓库根——建议先校验全量再写或失败回滚。
- **T25【提示】CHANGELOG 历史条目风格不统一**：历史缺口（[0.4.4]、[0.3.30]、[0.3.20–0.3.22] 无条目但对应 tag/发布存在）；≤v0.3.19 条目带 `v` 前缀、≥0.3.23 无前缀。建议后续条目统一格式，缺口按需补录或注明。

## 6. 修复优先级清单（建议顺序）

1. **立即（功能失效 / 进程误杀 / 数据与凭据安全）**
   - 2.2 停止/端口清剿的进程身份校验（防误杀无关进程）；2.1 npm registry 镜像接通或从 UI 下线；
   - 2.3 版本排序改 semver + 工具链门槛比较产出 mismatch（含前端 I8/后端排序一致化）；
   - 2.4 GitHub PAT（DPAPI/凭据库、脱敏回传、git 认证走环境变量）；2.5 dsh web token 日志打码；
   - T1 CI 测试/`--all-targets` 覆盖；T5/T6/T7 Release 流程 tag 落点与并发/skip-release 语义修正。
2. **尽快（可靠性 / 竞态 / 盲区）**
   - 2.6 配置 RMW 加锁 + 原子写；2.7/2.8 子进程/网络超时与取消；2.9/2.10 启动/托盘失败降级不 panic；
   - 2.11（=T8）后端 5s 状态 reconcile；T2 vite.config.ts 类型检查 + @types/node；
   - 前端 M1 面板宽度 reclamp 联动、M2 日志流增量格式化；
   - S1 最小 CSP；S3 下载物 sha256 校验。
3. **排期**
   - 2.12 Python 目录版本排序；S4 版本串白名单校验；S6 卸载 DSH_HOME 二次确认；
   - 前端 L1–L9（定时器/轮询取消令牌/useTauriListener/滚动与导出细节）；
   - T9（=2.3 mismatch UI）、T10 停止时序文档对齐、T11 文档批量对账、T12/T17/T18/T19 依赖归类/命名/脚本修复。
4. **随手（低）**
   - 2.13/2.14 死代码/重复/占位/注释漂移清理（global_shim_is_ours、decode_line、URL 提取合并、PATH 注入合并、`let _ = round;`）；
   - 前端 I1–I13（死导出、index.css 重复块、死主题、use client 噪声、版本平局比较等）；
   - T13–T16/T20–T22/T24/T25 依赖元数据、GITHUB_OUTPUT 写法、capabilities 收敛、文档/CHANGELOG 格式统一。
5. **完成后回归**：`npx tsc --noEmit`、`npx tsc -p tsconfig.node.json --noEmit`、`cargo check --all-targets`、`cargo test --lib`、4 个离线集成测试、`npm run build`。

---

## 7. 修复与优化执行记录（全链路修复阶段 · 待版本号递增后成为 v0.4.13）

> 本节为按本报告执行的代码修复清单（工作区 37 个变更文件 + 本报告），全部经本地验证：
> `tsc --noEmit` ✅ · `tsc -p tsconfig.node.json --noEmit` ✅ · `npm run build` ✅ ·
> `cargo check --all-targets`（0 警告）✅ · `cargo test --lib` 35/35 ✅ ·
> 6 个离线集成测试（config_default / tray_icon / concurrency / icon_window / channel_mutex / path_inject）✅

### 7.1 已修复（按报告条目）

**后端（Rust）**
- **2.1 npm registry 接线**：`core/config.rs` 新增 `npm_registry_url()/current_npm_registry()`；已注入
  `commands/version.rs`（npm view / npm install -g）、`commands/toolchain.rs`（npm i -g pnpm）、
  `core/github.rs`（GitHub 通道 pnpm install）。
- **2.2 端口误杀防护**：`core/process.rs` 新增命令行身份判定 `cmdline_looks_like_dsh()`（+单测）；
  `adopt_running()` 改为先校验监听进程再收养（非 dsh 仅警告不接管）；`stop_locked()` 按端口反查 PID
  前校验；端口清剿循环只强杀 dsh 形进程（落实此前 `let _ = round;` 占位缺失的身份比对）。
- **2.3 semver 排序 + mismatch**：`core/github.rs` 标签排序改 semver 数值比较（跨 10 位与多位数预发布
  排序回归单测）；`commands/toolchain.rs` 增加版本解析与门槛比较（Node 22.19+/24+、Git 2.26+、
  Python 3.10+），detect 产出 `present/missing/mismatch`（+单测）；前端 ToolchainPanel 已有 mismatch 徽标。
- **2.4 GitHub PAT**：`core/config.rs` Windows DPAPI（`dpapi1:`+hex 前缀）落盘加密、load 解密、解密失败
  置空；git 认证改 `GIT_CONFIG_*` 环境变量注入（不再进子进程命令行）；`ConfigView` 新增
  `github_token_set`、不再回传明文 token，`set_github_token` 空串=不修改；前端类型与
  SettingsPanel（掩码占位、留空不变）同步。
- **2.5 dsh token 日志打码**：`core/logging.rs` 新增 `redact_web_token()`（+单测）与独立缓存
  `last-web-url`（save/clear/extract 优先读缓存）；tail 捕获原始 URL 入内存+缓存、落盘/推送一律打码；
  stop 与进程退出均清理缓存。
- **2.6 配置竞态**：set_* 持进程级 `cfg_write_lock()`；`save()` 改临时文件+rename 原子写。
- **2.7/2.8 超时护栏**：`core/command.rs` `run_with_timeout()`（并发读管道+超时杀进程树）；
  `core/stream.rs` 看门狗（30min 总上限）+ 读线程带宽限 join；远端/下载/UAC 调用点全部接入
  （git ls-remote 90s、npm view 120s、下载 600s、解压 180s、UAC -Wait 1200s、端口查 PID 15s 等）。
- **2.9/2.10 启动/托盘降级**：`Logger::init()` 目录降级不再 panic；`setup_tray()` 失败仅落日志跳过托盘；
  Tauri run 错误 eprintln+exit(1)。
- **2.11 后端 5s reconcile**：`spawn_reconcile()/reconcile_once()`（收养实例退出/端口易主收敛、外部启动
  自动收养），与 2.2 身份校验复用。
- **2.12 Python 目录排序**：`python_dir_ver()` 数值比较（Python310 > Python39），+单测。
- **2.13 死代码/重复**：删除 `global_shim_is_ours`/`shim_path_is_ours`、`text::decode_line`；PATH 注入
  合并 `pathutil`；`run_cmd` 回退走 `hidden_cmd`；AGENTS.md 悬空引用与多处注释漂移清理。
- **2.14**：monitor 收尾状态改为 pid 归属条件复位（防覆盖重启新实例）；测试 `unused_mut` 警告修复。

**前端**
- **M1**：`AppShell.tsx` 接线对侧开关变化 → `reclampWidth()`（此前无调用方）。
- **M2/L6/L7/L9**：`LogPanel.tsx` 重写——每条日志仅格式化一次（StreamEntry）+ `useMemo`；贴顶滚动
  （非贴顶不回顶）；删除死 prop `onClose`；导出 blob 延迟回收与文件名清洗。
- **L1**：`VersionPanel.tsx` 进度清理定时器登记 ref 并卸载清理。
- **L2**：`StatusCard.tsx` `waitForWebUrl` 接收取消令牌。
- **I8**：`version.ts` 平局改 semver 预发布数值比较（rc.9 < rc.10）。
- **I11**：`SettingsPanel.tsx` 开关基于最新快照（ref）；token 输入不回显明文。

**构建 / CI / 配置 / 脚本 / 文档**
- **T1**：`ci.yml` cargo check 加 `--all-targets`，集成测试补 channel_mutex / path_inject；
  网络/环境型测试 `#[ignore]`。
- **T2**：加 `@types/node`；修 `vite.config.ts`；CI 增加 `tsc -p tsconfig.node.json --noEmit`。
- **T5/T6/T7**：`release.yml` 推送后显式打 tag、决策前同步远端 master、skip-release 不再自动递增。
- **T12**：构建期/CLI 依赖移入 devDependencies，lockfile 重生成。
- **T18**：`verify-layout.py` 顶部标注已失效。
- **文档对账**：CONTEXT / DESIGN / ADR-0004 / README / bump-version.mjs 注释 / .gitignore（tsbuildinfo）。

### 7.2 已知未处理 / 建议后续（含原因）
| 条目 | 说明 |
|---|---|
| S1 CSP=null | 未启用全局 CSP：需真实 dsh GUI 内嵌窗口回归后决定（远程窗口已按 capability 无 IPC 隔离）。建议后续配最小 CSP 并实测。 |
| S2 capabilities 收敛 | 收敛需运行期回归确认前端窗口 API 实际使用面，未在此批改动。 |
| S3 下载校验和 | 未实现：镜像/版本升级会使固定校验和漂移，建议后续按官方 SHASUMS 校验。 |
| S6 DSH_HOME 删除二次确认 | 已有目录特征校验；UI 二次确认为产品交互决策。 |
| T17 便携包命名统一 | 避免破坏既有下载约定，需发布策略拍板。 |
| T18 verify-layout.py | 已标注失效；重写/删除待定。 |
| favicon 生成 | 未纳入 generate-icons.ps1 清单（低）。 |
| release.yml tag 流程 | 代码级修复完成，需一次真实发布验证。 |
| DPAPI 读写往返 | 编译已通过；建议真实会话"保存→重启读回"验证。 |

### 7.3 发布前建议回归
- `npm run tauri dev` 手工冒烟：启动/停止/重启、双通道安装（验证 registry 镜像生效）、内嵌/外部打开、
  托盘、Settings 保存 Token 后重启（DPAPI 读回）、旧版明文 token 配置兼容（load 对非 `dpapi1:` 前缀按明文处理）。
