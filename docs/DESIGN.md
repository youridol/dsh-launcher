# dsh-launcher 设计总览

> 基于三轮 grilling 的最终设计。决策依据见 `CONTEXT.md`（词表）与 `docs/adr/`（ADR-0001~0004）。

## 1. 产品定位

**dsh-launcher** 是 deepseek-harness（`dsh`）的 Windows 桌面启动器与管理器：

- **不改 dsh 源码**，只复用官方 CLI/接口（`dsh web`、SIGTERM 信号协议、npm 包、GitHub Releases）。
- **全局单版本模型**：dsh 与工具链都是全局安装、单一版本；切换版本 = 先卸载再装。
- **职责收敛**：安装/卸载/更新、工具链检测与一键补齐、生命周期控制、日志全域、Web GUI 内嵌。

## 2. 技术栈（最终裁定）

| 层 | 选型 | 职责 |
|---|---|---|
| 后端 | **Rust（Tauri 2）** | 进程生命周期、端口探测、工具链检测/安装、日志落盘、配置持久化、版本/通道管理、事件推送 |
| 前端 | **TypeScript + React + shadcn/ui（黑暗主题）** | UI 渲染、配置表单、版本列表、日志流、托盘菜单、WebView 内嵌 |
| 桌面 | **Tauri 2（WebView2）** | 窗口、托盘、系统集成；**抛弃 Electron**（Q28 裁定） |
| 分发 | **NSIS 安装包 + 便携版（zip）** | Windows 两种分发形态 |

## 3. 模块划分（Rust 后端）

```
src-tauri/src/
├── main.rs               # Tauri 入口
├── commands/             # IPC 命令（前端调用）
│   ├── dsh.rs            #   dsh 生命周期：start/stop/restart/status
│   ├── toolchain.rs      #   工具链：detect/install/mirror
│   ├── version.rs        #   版本/通道：list/install/update/uninstall
│   └── logs.rs           #   日志：stream/filter/export
├── core/
│   ├── process.rs        #   spawn dsh、PID 跟踪、SIGTERM/taskkill
│   ├── port.rs           #   端口探测（启动前预检 + 5s 轮询）
│   ├── install.rs        #   npm 通道 + GitHub 通道 + 离线包安装
│   ├── toolchain.rs      #   工具链检测（node -v/git --version/…）与安装
│   ├── mirror.rs         #   镜像源配置（npm registry / GitHub 加速）
│   ├── config.rs         #   启动器配置持久化
│   └── backup.rs         #   DSH_HOME 手动备份/导出
├── logging.rs            #   dsh stdout/stderr + 启动器日志统一落盘
└── events.rs             #   Tauri event 推送（状态变更/日志流）
```

## 4. 核心流程

### 4.1 工具链检测与补齐
1. 检测清单：Node.js(22.19+/24+)、npm(随 Node)、pnpm、Git(2.26+)、Python(可选)。
2. 状态：`present` / `missing` / `mismatch`；缺失或版本不符 → UI 列出 + "一键安装"。
3. 安装走**官方通道**、**系统全局**；需提权的步骤（Git 装 Program Files）spawn `runas` 子进程，主进程不常驻管理员。
4. 网络问题由镜像源解决（npm registry / GitHub 加速独立配置，默认官方源 + 常用镜像下拉）。

### 4.2 dsh 安装（三入口）
- **npm 通道**：`npm i -g @deepseek-ai/dsh@<version>`（registry 现有版本，当前 0.1.1-rc.2）。
- **GitHub 通道**：从 GitHub Releases 拉 v0.1.2-alpha.1 及更新的源码 tag → clone → `pnpm install` + `pnpm build`。
- **离线包**：用户手动下载的 dsh 源码 tar.gz/zip → 选择文件 → 解压 → 本地构建（工具链仍需在线）。

### 4.3 生命周期
- **启动**：spawn `dsh web --port <p>`，CWD = dsh 官方默认（不干预 workspace root），注入全局工具链 PATH。
- **停止**：SIGTERM（Windows 下 taskkill /PID /T）→ 优雅排空 ≤5s → 二次信号强杀。
- **重启**：停止后用相同配置（端口 + CWD）重启；端口被占则提示手动改。
- **状态检测**：事件驱动（进程句柄退出即推送）+ 端口探活 5s 兜底轮询。

### 4.4 日志
- 采集：dsh stdout/stderr + 启动器自身日志，统一时间戳格式。
- 落盘：按天轮转 + 按大小 10MB 切割，保留 30 天。
- UI：实时流式 + 来源/级别过滤 + 关键词搜索 + 一键导出 .log。

### 4.5 窗口与托盘
- 内嵌 WebView2 渲染 `http://127.0.0.1:<port>` + "外部浏览器打开"兜底按钮。
- 托盘菜单：打开主窗口 / 启动 / 停止 / 重启 / 退出。
- 滑动开关：① 主窗口关闭 = 直接退出（含 dsh）② 最小化到托盘 ③ 卸载时保留 DSH_HOME（默认保留）。
- 退出：托盘"退出"先优雅停止 dsh 再退出；可勾选"退出时驻留 dsh"。

## 5. 数据与配置

- 启动器配置：`%APPDATA%\dsh-launcher\config.json`（端口、镜像源、滑动开关、通道偏好）。
- 日志：`%LOCALAPPDATA%\dsh-launcher\logs\`（按天目录，10MB 切割，30 天保留）。
- DSH_HOME：**只读展示、不接管**（`%USERPROFILE%\.dsh`，实际以 dsh 为准），卸载时默认保留、可开关。
- 手动备份：导出 DSH_HOME zip 到用户指定位置。

## 6. 版本策略（ADR-0004）

- 启动器版本独立于 dsh，从 **0.1.0** 起，**0.0.1 递进、0.1.9→0.2.0 十进一**（非语义化版本）。
- 三处同步：`package.json` + `src-tauri/Cargo.toml` + `src-tauri/tauri.conf.json`。
- `CHANGELOG.md` 顶部插中文条目。

## 7. 分发

- **NSIS 安装包** + **便携版 zip**。
- DoD：全流程冒烟（检测→装工具链→装 dsh→启动→内嵌 web UI 可交互→停止→卸载）在干净 Windows 跑通；日志导出含 dsh+启动器全量；NSIS 与便携版均可安装/运行。

## 8. 开放事项（实现前确认）

- Tauri 2 的 WebviewWindow 内嵌是否满足 dsh web GUI 全部交互（如有兼容问题退化为外链浏览器）。
- `taskkill /T` 对 Node 子进程树（pnpm 构建产物运行）的优雅排空验证。
- 提权子进程（runas）在 Windows 不同版本上的 UAC 行为一致性。
