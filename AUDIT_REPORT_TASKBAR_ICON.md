# dsh-launcher 任务栏图标模糊 + 全链路代码审计报告（严格模式，v0.5.0）

- 审计基线：HEAD `281aea8`（v0.4.14）+ 本批未提交改动
- 审计日期：2026-09-04
- 审计方法：主代理逐文件通读核心源码 + 三个并行子代理（Rust 后端全量 / 前端 TS 全量 /
  Windows 任务栏图标机制考证）+ 对锁定依赖（tao 0.35.3、tauri 2.11.5、tauri-runtime-wry
  2.11.4、tauri-codegen 2.6.3、wry 0.55.1、@tauri-apps/api 2.11.1，本地 registry/node_modules
  源码）逐行核实 + 官方文档/issue 联网考证；非仅凭经验。
- 修复/回归：tsc 0 错误、vite build 通过、cargo check --all-targets 0 警告、
  cargo test --lib 36/36、全部离线集成测试通过、cargo build --release 通过。

---

## 1. 总体结论

- 代码质量 **良好偏上（B+）**：分层清晰、修复纪律强（大量 v0.4.x 审计注释与回归测试）、
  凭据处理成体系、IPC 契约前后端一致；无高危可利用注入。
- 本次审计确认 **1 个真实用户可见缺陷**（前端入口内嵌窗口任务栏图标模糊，核心审计目标，
  机制 + 根因详见 §2）、**1 个高危用户环境破坏缺陷**（PATH 变量引用固化，§4.1）、
  **1 个安全隐患**（token 加密失败静默明文落盘，§4.2）、若干可靠性缺陷（§4.3-§4.8）。
- 对"审计面扩大"的发现：按严重度归档于 §5（已修/未修均列）。

---

## 2. 核心审计目标：任务栏图标模糊——机制与根因（源码级证据链）

### 2.1 Windows 任务栏图标机制（考证结论，附出处）
1. tao `Window::set_window_icon` / builder `with_window_icon` 在 Windows **只发
   `WM_SETICON(ICON_SMALL)`**；`ICON_BIG` 只由独立方法 `set_taskbar_icon` /
   `WindowBuilderExtWindows::with_taskbar_icon` 发送。Tauri 2 **从不调用后者**。
   → `tao-0.35.3/src/platform_impl/windows/{window.rs:882-903, icon.rs:117-126}`；
   `tauri-runtime-wry-2.11.4/src/lib.rs:1245,2410,3565`；tao 0.26/0.30.2/0.31.0 行为一致
   （本地比对）；docs.rs/tao/0.30.2（set_window_icon=ICON_SMALL、with_taskbar_icon=ICON_BIG 上限 256）。
2. 无显式 AppUserModelID 时，Shell 按 exe 自动派生 AppID → 同进程顶层窗口默认**同一任务栏
   组**；合并态按钮图标 = **组图标**，来源 ①开始菜单快捷方式→②桌面快捷方式→③exe 资源
   （RT_GROUP_ICON 多帧 .ico，Shell 按 DPI 精确取帧，锐利），与窗口 WM_SETICON 无关。
   → Raymond Chen 2015-08-12（devblogs.microsoft.com/oldnewthing/20150812-00）等。
3. 窗口自身条目（不合并/展开/预览）图标 = 窗口图标回退链：WM_SETICON 显式设置 → class
   icon → LoadIcon 默认。tao 注册窗口类 hIcon/hIconSm 均为空 → 未设图标窗口退回默认应用图标。
4. 任务栏按钮**取图通道是 WM_GETICON**（Explorer 主动拉取），WM_SETICON 只是更新窗口内
   存储；LWJGL 提交注释实证任务栏图标通常延迟约 100ms 才经 WM_GETICON 刷新，事件循环忙则
   更久；Win11/虚拟化环境有"不刷新"报告。运行期后置发送无刷新保证。
   → LWJGL commit 6fd210fdb；微软 Q&A #712281。
5. tauri 官方 issue #14596（Windows window icon blurry）实锤：`tauri-codegen` 读 .ico
   只取 `entries()[0]`（**首帧**）→ 单张 RGBA → tao `CreateIcon` 单尺寸 HICON → 系统对
   单帧做 GDI 缩放（无多帧可选）→ 首帧 16/32px 被放大即糊；即便 256px 大帧缩小同样有损。
   同一 .ico 在 exe 资源中由 Shell 多帧精确取图 → Explorer/组图标锐利。关联 PR #15241
   （codegen 选最大 entry）、#15274（Windows 从 app 资源读 default_window_icon）。
   → github.com/tauri-apps/tauri/issues/14596
6. ICON_BIG 语义尺寸 = SM_CXICON（系统大图标 ≈32px @96DPI）；任务栏目标物理尺寸随 DPI：
   100%→24、150%→36、200%→48、400%→96（微软社区数据）。256px HICON 缩到 24~48 属 ≥4x
   下采样，**不保证清晰**（GDI 缩放对细线/alpha 有伪影）。

### 2.2 本仓库代码路径（双入口不一致）
| 入口 | 窗口创建 | 图标 | 首帧 |
|---|---|---|---|
| 主窗口 | tauri.conf windows → 默认 "main" | setup 主线程同步 `apply_window_icon`（512 SMALL + 256 BIG） | 高清 |
| 内嵌（桌面快捷方式/自动打开） | Rust `WebviewWindowBuilder`（lib.rs） | builder `.icon(512px PNG)` + build 后立即 apply | 高清 |
| 内嵌（前端"内嵌打开"按钮，StatusCard） | JS `new WebviewWindow(label,{url,title,width,height})` | **无 icon**（JS WindowOptions 无 icon 字段，源码核实）→ 默认窗口图标 = tauri-codegen 首帧（本仓库 icon.ico **首帧 16×16**，实测解码）；`tauri://created` 后才 invoke `set_web_gui_icon` 补发 SMALL+BIG | **16px 默认图标，且补发晚于按钮首帧/WM_GETICON** |

### 2.3 根因结论
前端路径创建的内嵌窗口，任务栏按钮首帧以 **16px 默认图标低清放大**绘制（机制 3/5），
后置 `tauri://created → IPC → 同步 CreateIcon/SendMessageW` 的补发**必然晚于按钮建立与
WM_GETICON 取图**，且无触发重刷保证（机制 4）；主窗口与 Rust 入口在按钮建立前已完成
高清设置 → 清晰。**同一图标引擎、两套时序不同的入口 = 现象差异来源。**

### 2.4 修复（已实施，v0.5.0）
1. **统一创建路径**：新增 `create_web_gui_window` IPC 命令，内部在 Tauri 主线程用
   `WebviewWindowBuilder`（**预置 512px PNG** + 创建后同步 `apply_window_icon`
   SMALL+ICON_BIG 256px）创建窗口；前端"内嵌打开"改调之（删除 `new WebviewWindow`）。
   与桌面快捷方式/自动打开共用同一 `create_embedded_web_gui_window`（lib.rs 迁移至
   commands/dsh.rs）——三条入口创建逻辑一致，任务栏图标源与设置时机对齐主窗口。
2. 图标/创建失败经 **Logger 落警告**（此前仅 eprintln，release 无控制台不可见）。
3. label 追加进程内计数 + 时间/pid 混合后缀，杜绝极端同毫秒冲突。
4. `set_web_gui_icon` 保留为幂等兜底（不依赖也安全）。

**说明**：考证表明"任务栏组按钮图标由 exe/快捷方式多尺寸资源决定"——若任务栏设置
"始终合并"，两窗口本应显示同一锐利组图标；用户观察到差异说明按钮处于显示窗口条目或
Shell 未合并路径。本修复保证两窗口窗口级图标源与时序一致（不再有 16px 默认帧参与），
是代码侧能做的完整修复。组级 AppID/图标控制涉及任务栏交互语义（多窗口合并 vs 独立），
见 §6 S-1 供后续产品决策，不属于"模糊修复"必需。

### 2.5 需人工复核
视觉验证（本环境提供构建 + 集成测试 + 静态证据）：真实桌面启动器 → "内嵌打开" →
任务栏内嵌窗口图标应与主窗口一致清晰、无先模糊后跳转。若个别机器仍显示旧图标缓存，
重启 Explorer 后即恢复（系统级图标缓存行为）。

---

## 3. 回归验证（已执行）

| 项 | 结果 |
|---|---|
| `npx tsc --noEmit` | ✅ 0 错误 |
| `npx vite build` | ✅ 通过（bundle 406KB / gzip 127.6KB） |
| `cargo check --manifest-path src-tauri/Cargo.toml --all-targets` | ✅ 0 警告 |
| `cargo test --lib` | ✅ 36/36 通过（新增 tasklist 解析单测） |
| 集成测试 config_default / tray_icon / concurrency / icon_window / channel_mutex / path_inject | ✅ 全部通过 |
| `cargo build --release` | ✅ 通过 |
| 版本一致性（package.json/Cargo.toml/Cargo.lock/tauri.conf/package-lock） | ✅ 全部 0.5.0 |
| CHANGELOG 顶部条目 | ✅ 0.5.0 中文条目 |

---

## 4. 已修复问题明细

### 4.1【高危·用户环境破坏】用户 PATH 变量引用固化
`core/pathutil.rs::user_path()` 用 `RRF_RT_REG_EXPAND_SZ` 读取注册表 Path 时**自动展开
%VAR%**，`prepend/remove_user_path_dir` 把展开后的绝对路径写回 → 用户 PATH 中
`%SystemRoot%`/`%JAVA_HOME%`/`%USERPROFILE%` 等变量引用被永久固化为展开值、可能超长。
修复：改用 `RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND` 读原始字面值并字面写回；删除废弃
`expand_env`。影响面：Node 安装/卸载写用户 PATH 的唯一路径（toolchain.rs），此前任何一次
操作都可能固化变量 —— 用户若已受影响需手动还原（本条防增量破坏）。

### 4.2【安全】GitHub Token DPAPI 加密失败静默明文落盘
`core/config.rs::save` 原 `encrypt_token(token).unwrap_or_else(|| token.to_string())`：
CryptProtectData 失败时把 PAT **明文**写盘且无提示。修复：失败返回 Err（提示保存失败），
绝不降级明文持久化。

### 4.3【可靠性】日志 10MB 切割静默失效
写入句柄默认无 FILE_SHARE_DELETE，与 tail 只读句柄共存时 rotate 的 rename 触发
ERROR_SHARING_VIOLATION → 切割静默失败、日志无限增长。修复：写句柄显式
`share_mode(READ|DELETE)`（core/logging.rs）。

### 4.4【可靠性】tasklist 存活探测误判
`process_alive` 用 `text.contains(&pid.to_string())`：任务行其它列（内存/时间）恰含该数字
子串时误判存活（stop 等待可能提前放弃）。修复：抽 `tasklist_has_pid` 按行解析第 2 列精确
匹配 + 单测覆盖回归场景。

### 4.5【可靠性】超时强杀后等待退出无上限
`run_with_timeout` / `run_streamed` 超时强杀后 `try_wait` 死循环无护栏（taskkill 失败且进程
不退 → 永久卡死调用线程）。修复：3s 上限后尽力返回（command.rs / stream.rs）。

### 4.6【UX 一致性】重启前不保存端口
StatusCard `handleRestart` 此前不 savePort：UI 显示新端口、Rust `restart` 用旧端口重启 →
误导。修复：重启前先保存输入端口。

### 4.7【前端性能】日志补流全量解析 + 大量重复
LogPanel `backfillLatestFile` 此前对"最新文件"（常为 dsh-web-stdout.log，进程落盘输出，
可数 MB~10MB）全量 readLog + 解析，内容已实时进 log://line 流 → 补流几乎全重复 + 首帧卡顿。
修复：只补流 `yyyy-MM-dd.log` 日期日志、只保留尾部 MAX_STREAM_LINES 行。

### 4.8【死代码/清理】
- SettingsPanel 非 embedded 外壳分支（全仓无调用方，恒 embedded）→ 移除外壳与多余 import。
- pathutil 废弃 `expand_env`（RRF_NOEXPAND 后无调用）。
- 注释纠错：dsh.rs "icon_forensics 取证测试"（实际不存在）→ 改为指 tests/icon_window_test.rs。

---

## 5. 审计发现归档（未修项 + 说明 / 低优先项）

### 5.1【中】归并入 §4 之外的 Rust 后端项（评估后本轮未改，理由附后）
| # | 发现 | 说明 / 建议 |
|---|---|---|
| R1 | Mutex `lock().unwrap()` 30+ 处无 poison 恢复，持锁线程 panic 会连锁崩溃 | 热路径无 unwrap、持锁内无易 panic 操作；建议后续统一 `into_inner` 策略（低优先） |
| R2 | `AppConfig::load()` 高频全量读盘（状态轮询/命令频繁） | 建议加进程内缓存 + 写时失效（中优先） |
| R3 | 文本解码只覆盖 936/1252/437/866，日/韩/俄（932/949/1251）按 1252 解仍乱码 | 建议 MultiByteToWideChar(CP_ACP) 一次到位（中优先） |
| R4 | 日志 cleanup_old 仅 init 一次，常驻数月后 30 天保留失效 | 建议定时器周期清理（低优先） |
| R5 | 日志文件名按 UTC 日界（国内 08:00 前归前一天） | 需本地时间换算；影响小（低优先） |
| R6 | 内嵌窗口导航全放行 + 远程内容可达（capability 仅放行 opener）——远程窗口对自定义命令的调用实际被 ACL 拦截（本地核实：非 local 来源 invoke 强制 ACL，`webview/mod.rs:1823`；本应用无 app ACL，除 opener 全拒） | 无需修复；但若日后给 dsh-web-gui capability 增命令，须逐项评估远程可调面 |
| R7 | `python_install_dirs`/detect 多进程串行，每次检测数秒 | 建议并行化（低优先） |
| R8 | git/npm registry 查询等已带超时（v0.4.13 已修）；taskkill/where/Get-CimInstance 无超时 | 本机命令无网络风险，可接受 |
| R9 | 图标 RGBA→BGRA+AND mask 转换 3 处重复（dsh.rs / icon_window_test.rs / tao 内部） | dsh.rs 与测试各有用途，保留注释同步约束（低） |
| R10 | URL 提取 3 处近似实现（process/logging/tests） | 已各带单测；收敛需统一 parser（低） |

### 5.2【中/低】前端项
| # | 发现 | 说明 |
|---|---|---|
| F1 | 状态同步三方并存（5s 轮询 + 事件 + 手动 refresh），无集中 store；卸载/安装事件风暴 | 建议集中订阅层或 Rust 增推状态事件（中优先） |
| F2 | StatusCard 584 行职责过载 | 建议抽取 openWithGuide hook（低优先） |
| F3 | 若干 `toast.error(\`…: ${e}\`)` 拼接原始错误串 | 建议统一 errMsg 提取（低） |
| F4 | LogPanel 每条 log://line O(2000) 数组复制 | 高频日志下可优化为环形缓冲（低） |
| F5 | AppConfig.githubToken 字段恒空串但仍在前端类型中 | 建议标 deprecated（低） |
| F6 | ui 组件导出若干无使用（CardFooter/DialogTrigger 等 shadcn 样板） | 即用即删策略，未强改避免范围膨胀（低） |
| F7 | index.css 存在重复选择器段 | 可合并（低） |

### 5.3【低】机制性/架构
| # | 发现 | 说明 |
|---|---|---|
| A1 | 窗口事件策略在 core/tray.rs、窗口创建在 commands/dsh.rs、装配在 lib.rs——图标/窗口职责横跨三层 | 注释与模块边界基本自洽，非缺陷 |
| A2 | Logger 同时承担落盘 + event 发射 + 进度（events 语义挂 Logger 上） | 为少传 AppHandle 的务实取舍 |
| A3 | release.yml bump 流程复杂（tag/CHANGELOG/notes 多步） | 已多次迭代稳定，真实发布验证一次即可 |

---

## 6. 后续建议（产品/工程决策项，非本次必改）
- **S-1 任务栏组图标策略**：若产品希望内嵌窗口与主窗口在同一任务栏组、按钮显示 exe 多尺寸
  图标，可显式设 `System.AppUserModel.ID`（SHGetPropertyStoreForWindow）或将内嵌窗口作为
  主窗口的 owned/child 窗口；若希望独立按钮，为各窗口设独立 AppID。当前修复（窗口级图标
  统一高清）在合并态下与组图标显示不冲突。
- **S-2 真高分方案**：若后续要求任意 DPI 下窗口条目极致锐利，可子类化窗口处理 WM_GETICON
  按 lParam(DPI) 从多尺寸 .ico 返回对应帧（替代单帧 256 CreateIcon）。当前 256px 原生 HICON
  在 100~200% 主流缩放下已足够（集成测试验证 256px 保真），属增强项。
- **S-3 CSP**：主窗口 csp=null，若后续引入远程内容需设最小 CSP（§5.1 R6 已说明远程窗口
  现状受 capability 约束）。

---
（本报告与 FIX_PLAN_ICON.md 对应；历史审计见根目录原 AUDIT_REPORT.md（v0.4.13 轮次）。）
