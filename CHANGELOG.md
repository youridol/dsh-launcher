# Changelog

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
