# Changelog

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
