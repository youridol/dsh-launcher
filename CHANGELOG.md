# Changelog

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


