# 全链路任务栏图标代码审计 + 修复（v0.5.0，严格模式）

审计基线：HEAD `281aea8`（v0.4.14）+ 未提交工作区改动；审计日期 2026-09-04。
审计方法：主代理逐行通读核心代码 + 三个子代理并行（Rust 后端全量 / 前端全量 / Windows
任务栏图标机制考证）+ **对依赖源码（tao 0.35.3 / tauri 2.11.5 / tauri-runtime-wry 2.11.4 /
tauri-codegen 2.6.3 / wry 0.55.1，均来自本地 cargo registry 与 node_modules）做第一手源码
级证据链核实**，非仅凭文档/经验。

---

## 1. 任务栏图标模糊根因（核心审计目标）

### 1.1 现象
- 启动器**主窗口**任务栏图标清晰；
- 内嵌 WebView2 子窗口（dsh Web GUI，`http://127.0.0.1:3080`，走前端"内嵌打开"按钮）**任务栏图标模糊**；
- 桌面快捷方式/自动打开入口的内嵌窗口则清晰（同进程、同一 apply_window_icon）。

### 1.2 依赖源码级证据链（逐一核实）

| # | 依赖事实 | 出处（本地源码） |
|---|---|---|
| A | tao `set_window_icon` **只发 `WM_SETICON ICON_SMALL`**；ICON_BIG 仅由独立 `set_taskbar_icon` 发送 | `tao-0.35.3/src/platform_impl/windows/window.rs:882-903` + `icon.rs:117-126`（`IconType::Small = ICON_SMALL`、`Big = ICON_BIG`） |
| B | tauri-runtime-wry `Window::set_icon` 与 builder `.icon()` 只透传到 tao `set_window_icon`（即只设 Small），**不发 ICON_BIG** | `tauri-runtime-wry-2.11.4/src/lib.rs:1245-1250, 2410-2418, 3565-3567` |
| C | tauri **Windows 默认窗口图标** = `icons/icon.ico`，经 `ico::IconDir::read` **取第 0 帧** 解码为 RGBA 注入 builder | `tauri-codegen-2.6.3/src/context.rs:211-244` + `image.rs:51-72`（`&icon_dir.entries()[0]`） |
| D | 本仓库 `icon.ico` 第 0 帧 = **16×16**（实测解码 16×16；帧序 16/24/32/48/64/128/256 全 PNG 压缩） | `pwsh` 实读 ICO 目录：entry[0] width=16 |
| E | 未显式 `.icon()` 的窗口：tauri `prepare_window` 自动补 default_window_icon（=上面 16px 图），该 16px 经 tao `into_windows_icon`→CreateIcon 成为窗口 ICON_SMALL HICON | `tauri-2.11.5/src/manager/window.rs:74-78` |
| F | JS `WebviewWindow` options 无 icon 字段；前端创建后 `tauri://created` 派发（此时窗口已默认可见） | `@tauri-apps/api@2.11.1` `window.d.ts`/`webviewWindow.d.ts`/`webviewWindow.js:67-68` |
| G | 本仓库内嵌窗口有**两条创建路径**：Rust `WebviewWindowBuilder`（lib.rs，桌面快捷方式/自动打开）与前端 `new WebviewWindow`（StatusCard"内嵌打开"），两者首帧图标源与设置时序不同 | 本仓库 lib.rs:106-158 / StatusCard.tsx（审计前 383-400） |
| H | 本仓库自定义 `apply_window_icon` 用 256px PNG→RGBA→CreateIcon 原生 HICON + WM_SETICON ICON_BIG（该路径经 `tests/icon_window_test.rs` 真实窗口端到端验证有效，本次亦回归通过：ICON_BIG 读回 256×256） | 本仓库 commands/dsh.rs + tests/icon_window_test.rs |

### 1.3 根因结论
- 主窗口清晰：tauri.conf.json 启动窗口在 **setup 主线程同步** `apply_window_icon`，早于任务栏按钮建立/首次绘制。
- 前端创建的 WebView2 内嵌窗口：创建时**无高清图标**（E+F）→ 任务栏按钮以 **16px 默认图标低清放大**绘制（C+D）；`tauri://created` 后才 invoke `set_web_gui_icon` 补发（IPC 往返 + IPC 线程同步 CreateIcon/SendMessageW）——**必然晚于按钮首帧**，且窗口 show 后 WM_SETICON 对任务栏按钮图标缓存的刷新在不同 Windows/Explorer 状态下不保证即时重绘。
- Rust 创建路径 builder 预置 512px（首帧高清）+ build 后立即 apply（A/B 已在窗口可见前完成）→ 清晰。
- **同一图标引擎、两条时序不同的入口 = 主窗口/部分内嵌清晰、前端内嵌模糊的差异来源**。

### 1.4 修复（已实施）
统一创建路径：新增 `create_web_gui_window` IPC（内部 `WebviewWindowBuilder` 预置 512px +
创建后同步 apply SMALL/BIG），前端"内嵌打开"改调之；桌面快捷方式/自动打开共用同一
`create_embedded_web_gui_window`。三条入口窗口创建逻辑完全一致，模糊分歧从机制消除；
图标/创建失败全部经 Logger 落警告（此前仅 stderr，release 不可见）。

### 1.5 运行期视觉复核（需人工）
本环境完成静态证据 + 构建 + 集成测试；任务栏视觉请在真实桌面复核：
启动器 → 点"内嵌打开" → dsh Web GUI 窗口任务栏图标应与主窗口一致清晰且无
"先模糊后跳高清"闪烁。若个别机器仍偶发旧图标缓存，重启 Explorer 后即恢复（系统级行为）。

---

## 2. 修复的其它高置信问题（已实施并验证）

1. **[高危·环境破坏] 用户 PATH 变量引用固化**：`pathutil::user_path` 用
   `RRF_RT_REG_EXPAND_SZ` 读取自动展开 `%VAR%`，prepend/remove 写回展开后绝对路径 →
   永久破坏 `%SystemRoot%` 等变量引用。改为 `RRF_NOEXPAND` 读原始值、字面写回；
   删除废弃 `expand_env`。
2. **[安全] GitHub Token DPAPI 加密失败静默明文落盘** → 改为返回 Err 拒绝明文保存。
3. **[可靠性] 日志 10MB 切割静默失败**（写句柄无 FILE_SHARE_DELETE，rotate rename 撞
   tail 读句柄 → ERROR_SHARING_VIOLATION）→ 写句柄共享 读|删除。
4. **[可靠性] `process_alive` PID 判定误判**（`contains(pid字符串)` 命中内存/时间列子串）
   → 按行解析第 2 列精确匹配 + 单测。
5. **[可靠性] 超时强杀后等待退出无上限** → taskkill 失败可死循环，加 3s 护栏（command/
   stream 两处）。
6. **[UX 一致性] 重启前不保存端口**（UI 新端口 vs Rust 旧端口误导）→ 重启前 savePort。
7. **[前端性能] LogPanel 补流全量解析 10MB+ 进程输出落盘文件且与实时流重复** →
   仅补流 `yyyy-MM-dd.log`、只取尾部 2000 行。
8. **[死代码] SettingsPanel 非 embedded 外壳分支（无调用方）**等清理。

## 3. 审计确认的其它发现（未修/说明，按优先级归档见 AUDIT_REPORT.md）
（已并入 AUDIT_REPORT.md 完整版：死代码/重复项清单、Mutex poison 策略、多代码页解码、
reconcile 与 stop 竞态收敛、cleanup_old 仅 init 一次、时间戳 UTC 日界、图标 RGBA 转换 3
处重复、URL 提取 3 处近似、commands 层与 capabilities 关系澄清等。）
