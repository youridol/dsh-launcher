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

- **dsh 运行状态**：`running`（端口可探活 + 进程存在）| `stopped`。由**端口探测 + 进程匹配**判定，无官方状态接口。
- **工具链状态**：`present`（版本满足）| `missing`（不存在）| `mismatch`（存在但版本不符）。可检测、可一键安装、可配镜像源。
- **工具链安装模式**：**系统全局**（官方通道安装，管理员权限可能触发 UAC）。无用户级目录选项。

## 运行目录

- **workspace root**：启动 dsh 时的 CWD，由 dsh 官方行为决定（默认该目录）。启动器**不配置、不干预、不校验**。

## 镜像源（镜像维度）

- **npm registry**：npm 通道装包 + pnpm 依赖安装的 registry。
- **GitHub 加速**：源码 clone（git 镜像）与 GitHub Release 下载（代理）共用。
- **Node/Git 二进制源**：工具链安装包的下载源。

## 生命周期

- **启动**：spawn `dsh web --port <p>`，注入工具链 PATH，CWD = 运行目录（dsh 官方默认）。
- **停止**：向 dsh 进程发送 SIGTERM（优雅排空，最多 5 秒），二次信号强制退出。**无官方 stop 命令**。
- **托盘**：关闭窗口→托盘；托盘菜单 = 打开主窗口 / 启动 / 停止 / 重启 / 退出。
- **滑动开关**：开关 1 = 主窗口关闭按钮直接退出（含 dsh）；开关 2 = 最小化到托盘；开关 3 = 卸载时是否保留 DSH_HOME（默认保留）。
- **退出**：托盘"退出"先优雅停止 dsh，再退出启动器；可勾选"退出时驻留 dsh"。
- **重启**：停止（SIGTERM）→ 用相同配置（端口 + CWD）重新启动；端口被占则提示手动改。
- **运行状态检测**：事件驱动（Rust 持有 dsh 子进程句柄，退出即推送）+ 端口探活每 5 秒兑底轮询。
