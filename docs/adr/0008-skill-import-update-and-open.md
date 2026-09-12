# ADR-0008 — 技能导入、手动检查更新与外部打开

- **状态**：Accepted（2026-09-11 实施完成：`core/skill/{import,source,update,editor}.rs`、`commands/skill.rs`、`commands/config.rs`、`core/config.rs`、`core/logging.rs`、`core/github.rs`、`SkillsPanel`）
- **日期**：2026-09-11
- **范围**：`dsh-launcher` 新增技能从 URL 导入、手动检查更新、来源注册表、`AGENTS.md`/`CONTEXT.md` 编辑入口、可配置外部编辑器；**不修改 `deepseek-harness` 任何代码**。
- **事实依据**：`deepseek-harness` 检出 `C:\Users\Administrator\AppData\Local\dsh-launcher\github-dsh\deepseek-harness`（版本 `0.1.5-rc.1`）。ADR-0007 已确立「官方无技能启停接口，唯一合规路径是改写 frontmatter」与「官方只发现扫描根顶层一层」两条地基，本文在其上扩展。

---

## Context

### 1. 官方发现规则使「克隆即用」不可能

官方 `skill-filesystem` README `:36,146` 原文：**"nested `**/SKILL.md` files are deliberately not discovered"**、**"Discovery is one level deep — only `<root>/<name>/SKILL.md` and `<root>/<name>.md` are recognized"**。

真实技能仓库普遍**分类嵌套**：例如 `mattpocock/skills` 的布局是 `skills/<分类>/<name>/SKILL.md`（两层）。直接克隆到技能根，顶层只有 `engineering/`、`productivity/` 两个目录，两者都不含 `SKILL.md` → **零个可发现技能**。

**结论**：任何「从 URL 拉取技能」都必须是**导入 + 扁平化**：浅克隆到临时目录 → 递归收集 `SKILL.md` → 校验 → 复制到 `<root>/<name>/`。

### 2. 目录名必须取 frontmatter 的 `name`

实测本机 93 个技能中有 2 个目录名与 `name` 不符（`composition-patterns` → `vercel-composition-patterns`、`react-best-practices` → `vercel-react-best-practices`）。照抄仓库目录名会让「磁盘上的名字」与「官方解析出的名字」不一致，产生歧义。

### 3. 技能是目录，不是单文件

实测 93 个技能中 **70 个含 `SKILL.md` 之外的兄弟文件**、**30 个含 `agents/` 子目录**（如 `domain-modeling/agents/openai.yaml`）。技能正文按相对路径引用这些兄弟文件（`CONTEXT-FORMAT.md`、`ADR-FORMAT.md`、`tests.md`…）。**只复制 `SKILL.md` 会得到「技能还在但引用全断」的静默损坏。**

### 4. 覆盖策略（Q29 A）：文件级

上游提供的文件覆盖；**上游没有的本地文件/目录原样保留**（`agents/`、本地新增 `.md`）；**上游已删除的文件不落地删除**。若改为「目录级替换」，本地独有内容会被静默删除。

### 5. 「检查更新」必须是纯手动、永不自动写盘

用户明确要求：**不做自动更新**，只提供手动「检查更新」按钮。因此全仓**不新增任何定时任务**（既有唯一周期循环是 `process.rs` 的 5 秒状态对账，本功能不接入）。检查产出三类清单（新增 / 覆盖更新 / 本地独有保留），**应用必须由用户二次确认**。

### 6. 编辑器打开：lib 不得触达 GUI 栈（本 ADR 最有价值的工程约束）

`tauri_plugin_opener::OpenerExt::open_path` 会把整个 GUI DLL 栈（`user32`/`gdi32`/`comctl32`/`dwmapi`/`shcore`/`uxtheme`/`ole32`/`oleaut32`…）链入**库**。后果在 `cargo test --lib` 上暴露：unittest 二进制被链成 GUI 程序，它没有 side-by-side manifest，加载到 v5 的 `comctl32.dll`，缺少 `SetWindowSubclass`/`TaskDialogIndirect` 等 v6 导出 → **`STATUS_ENTRYPOINT_NOT_FOUND (0xC0000139)`，测试二进制根本无法启动**。

更早的根因是 `Logger` 直接持有 `tauri::AppHandle` 字段：任何 `#[cfg(test)]` 代码构造 `Logger` 都会把 GUI 栈拖进测试二进制。本文将其修复为**类型擦除的 emitter**（见下）。

---

## Decision

| # | 决策 | 关键理由 |
| --- | --- | --- |
| **D1** | 导入 = **递归扁平化**（浅克隆 → 递归找 `SKILL.md` → 复制到 `<root>/<name>/`） | 官方只发现一层，仓库普遍嵌套 |
| **D2** | 目标目录名取 frontmatter `name`，不取仓库目录名 | 本机 2 例实证：目录名可与 name 不符 |
| **D3** | **整目录复制**（含兄弟资源文件与 `agents/` 子目录） | 技能是目录；只复制 SKILL.md 会断引用 |
| **D4** | 覆盖 = **文件级**（Q29 A）：上游有则覆盖、本地独有保留、远程删除不落地 | 保护本地产物；删除是破坏性动作只提示 |
| **D5** | **只支持手动「检查更新」**，无定时任务、无自动写盘 | 用户明确要求；第三方提示词变更风险高 |
| **D6** | 检查判定 = **逐文件内容比较**，非 commit 比较 | commit 变了可能只改 README，技能文件没动 |
| **D7** | 来源记录落 `skill-sources.json`（**不叫 `skills.json`**） | `skills.json` 已被退役的 ADR-0005 共享模块占用 |
| **D8** | `AGENTS.md`/`CONTEXT.md` 编辑入口 = Rust 命令 + **闭集枚举**，前端不能传任意路径 | 结构上消除路径穿越；无需改 capability |
| **D9** | 打开方式退回链：配置的编辑器 → 系统默认程序（`ShellExecuteW`） | 编辑器失败不静默 |
| **D10** | **`Logger` 类型擦除 emitter**，不再持有 `tauri::AppHandle` | 修复 `--lib` 测试二进制被链成 GUI 程序而无法加载 |
| **D11** | 系统默认打开用 `windows` crate 的 `ShellExecuteW`，不用 `tauri_plugin_opener` | 后者拖入 GUI DLL 栈；前者只依赖 shell32 |
| **D12** | 首次点击编辑弹一次性引导（选系统默认或指定编辑器） | 用户明确要求「提示用户配置打开程序」 |
| **D13** | 镜像重写只对 `https://github.com/` 生效 | 对非 GitHub 地址套 GitHub 镜像前缀只会得到无效 URL |
| **D14** | 导入 UI = **批量面板**（仓库名标签 + URL + 「增加」追加条目 + 「确定」批量导入） | 用户需求：批量 github 仓库；单条失败不中断其余 |
| **D15** | `SourceRecord.name`（仓库标签）持久化，`display_name` 空则回退 URL | 来源列表以友好名展示 |

### D14/D15 精确契约（批量导入）

- 前端：`pendingRepos: {name, url}[]` 列表；「增加」追加（同 URL 去重）；「移除」单条；
  「清空」列表；「确定」批量导入（另有「预览」只读）。
- 后端：`import_batch(items: &[ImportItem], apply, logger) -> BatchImportReport`，逐条调用
  `import_from_url`，**单条失败（空 URL / 非 URL / 网络不可达）不中断其余条目**，
  聚合 `ok_count` / `failed_count` / 逐条 `items`。
- 仓库名 `name` 透传到 `SourceRecord.name`（空则 `None`），来源列表与检查更新以
  `display_name()`（name 优先，空回退 URL）展示。

### D1/D4 精确契约（导入与文件级覆盖）

对每个含 `SKILL.md` 的目录（递归发现，排除 `.git`/`node_modules`，技能目录内不再下钻）：

1. 读 frontmatter，取 `name` + `description`；缺任一 → 跳过并报原因（官方会丢弃该技能）。
2. `name` 须满足官方 `isSkillName`（`^[a-z0-9]+(?:-[a-z0-9]+)*$`），否则跳过。
3. 目标 = `<agentsHome>/skills/<name>/`；逐文件比较：
   - 本地无 → `Added`（新增）
   - 两边内容不同 → `Updated`（覆盖）
   - 相同 → `Same`
   - **仅本地有** → `LocalOnly`（保留，绝不删除）
4. 检查模式（`apply=false`）**绝不写盘**；应用模式才复制 `Added`/`Updated` 文件。

### D6/D7 精确契约（检查更新）

- 数据源 = `skill-sources.json` 里的 URL 列表。
- 对每个来源：浅克隆到临时目录 → 逐技能规划差异 → 汇总三类清单 → 清理临时目录。
- 报告 `actionable` 的技能数；`apply` 仍需用户确认后另调命令。
- 检查失败（网络/仓库不可达）记录在该来源的 `error` 字段，**不中断其余来源的检查**。

### D10 精确契约（Logger 类型擦除）

`Logger` 原字段 `emitter: Mutex<Option<tauri::AppHandle>>` 改为
`emitter: Mutex<Option<Box<dyn Fn(&LogEvent) + Send + Sync>>>`，其中 `LogEvent` 是
`Line(LogLine) | Progress(ProgressPayload)` 的内部枚举。`set_emitter` 在启动时把
`app.emit(...)` 封装进闭包；`emit_line`/`progress` 只调用擦除后的闭包。测试二进制里
`set_emitter` 是死代码、被链接器剥离，`tauri::AppHandle` 与 GUI 栈因此不进测试二进制。

### D8/D9/D11/D12 精确契约（外部打开）

- 前端只能传闭集枚举 `agents-md` / `context-md` / `skills-root`（路径由 Rust 从
  `dshhome` helper 推导），或某个受管技能文件路径（Rust 按 ADR-0007 D10 校验归属）。
- 文件不存在时按模板创建（`AGENTS.md`/`CONTEXT.md`）；目录不存在则创建。
- 退回链：配置的 `editor_command` 非空 → 用 `std::process::Command` 启动（引号感知
  切分，支持 `code --wait`）；失败 → 退回 `ShellExecuteW`；再失败 → 返回具名错误，
  前端提示去设置配置。
- `AppConfig` 新增 `editor_command: String`（空 = 系统默认）与
  `editor_prompt_seen: bool`；结构体带 `#[serde(default)]`，**旧 config.json 无需迁移**。

---

## Architecture

```
core/skill/
├── import.rs    —— 递归扁平化 + 文件级覆盖（plan_from_clone / apply_plan / import_from_url）
├── source.rs    —— skill-sources.json 来源注册表（schemaVersion 1）
├── update.rs    —— 手动检查更新（check_updates / apply_source_update）
└── editor.rs    —— 闭集目标解析 + 可配置编辑器 + ShellExecuteW 系统默认打开

core/logging.rs  —— Logger 类型擦除（D10）
core/github.rs   —— git_clone_command（任意仓库克隆，复用 git 解析/Token/镜像）
core/config.rs   —— editor_command / editor_prompt_seen
```

### 复用的既有基础设施

- git 可执行解析 / Token 注入 / 镜像重写：`core/github.rs`（`git_clone_command` 导出复用，
  不复制实现，也不改 dsh 仓库路径的既有行为）。
- 超时执行：`core/command.rs::run_with_timeout`（clone 180s、`rev-parse` 30s）。
- 事件广播：`core/events.rs::emit_skill_changed`（导入/更新后前端重扫）。
- 注册表容错姿态：照 `plugin/registry.rs`（缺文件/损坏 → 返回默认，不阻塞）。

### 安全边界

- **URL 校验**：只接受 `http(s)://`、`git@`、`ssh://`、`git://`、`file://`、本地路径；
  挡掉明显非 URL 的输入（空串、随机文本）。
- **本地来源（`file://` / 绝对路径 / UNC）的安全含义（审计 G6 补记）**：该白名单使
  「导入」可指向**本机任意 git 仓库**，进而把其内容复制进受管技能根（`<agentsHome>/skills`）
  —— 技能是会被 dsh 当作指令加载的目录。因此：
  1. 这是**用户显式发起的本机操作**（导入需二次确认；`apply=false` 为只读预览），
     与「读取任意路径」的文件浏览原语不同；
  2. 本能力**不新增攻击面**的前提是「IPC 不可被远程/不可信内容调用」：
     Tauri 对非本地来源的应用命令一律拒绝（见 `tauri-2.11.5` `webview/mod.rs` 的
     `!is_local` 来源守卫），故远程页面无法借此读取本机仓库；
  3. 若未来新增**非交互**调用入口（计划任务/远程触发），必须重新评估该白名单，
     或将本地路径限制在用户显式选择的目录内。
- **镜像重写限定 github.com 前缀**（D13）：对自建 Git 服务器/`git@`/本地路径绝不套用
  GitHub 镜像。
- **前端无法打开任意路径**（D8）：闭集枚举 + 受管根归属校验，与 ADR-0007 D10 同一信任边界。
- **不自动更新**（D5）：第三方提示词（SKILL.md 正文）会直接影响模型行为，静默变更风险
  不可接受；一切应用都经用户确认。

---

## Consequences

### 正面

1. 补齐「从 URL 导入」这一真实需求，且**不违反官方只发现一层的规则**（扁平化是导入器的
   职责，不是改官方行为）。
2. **零新依赖、零新 capability**：`ShellExecuteW` 复用既有 `windows` crate；编辑打开走
   Rust 命令不受 ACL 约束。
3. 修复了一个隐藏很深的工程缺陷（`Logger` 拖入 GUI 栈导致 `--lib` 测试二进制无法加载），
   让后续任何核心模块的单元测试都能自由构造 `Logger`。
4. 来源注册表与检查更新都是纯手动，不引入任何后台网络行为。

### 负面 / 代价

1. **覆盖式导入会覆盖用户已改的技能内容**（文件级，本地独有保留但不做内容 diff 合并）。
   缓解：应用前有「预览清单」需二次确认；备份策略见 ADR-0007（启停写盘才有备份，导入是
   整目录覆盖，当前不逐文件备份 —— 这是**已知边界**，须在 UI 文案里如实告知）。
2. 检查更新每次都浅克隆，对多来源/大仓库较慢（180s 超时上限）。
3. `skills.json` 与 `skill-sources.json` 两个文件并存，名字相近易混淆 —— 已在前者退役、
   文档与代码注释里反复标注。
4. 编辑器命令是自由文本，用户可填错；退回链（编辑器失败 → 系统默认）缓解，但错误的
   `editor_command` 会在每次打开时先失败一次再退回。

---

## References

- `$DSH_SRC/packages/skill/skill-filesystem/README.md:36,146`
- `$DSH_SRC/packages/skill/skill-filesystem/src/index.ts:909-935,992-1029`
- `$DSH_SRC/packages/skill/skill/src/index.ts:21,28,35-37`
- 本项目：`docs/adr/0007-skill-management.md`、`src-tauri/src/core/logging.rs`、
  `core/github.rs`、`core/skill/{import,source,update,editor}.rs`、
  `commands/skill.rs`、`commands/config.rs`、`core/config.rs`、
  `tests/skill_import_pipeline_test.rs`
