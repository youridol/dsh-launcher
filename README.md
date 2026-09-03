# dsh-launcher

[deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) 的 Windows 桌面启动器与运行环境管理器（Tauri 2 + React + TypeScript + shadcn/ui）。

## 功能

- **dsh 生命周期管理**：启动 / 停止 / 重启 `dsh web`（端口可配置，默认 3080），状态实时探测
- **双通道版本管理**：npm 通道（registry 装包）与 GitHub 通道（clone + pnpm 构建），最新版本置顶，安装进度实时展示
- **工具链一键安装**：检测 Node / npm / pnpm / Git / Python，支持镜像源（npm registry / GitHub 加速 / Node 二进制）
- **Web GUI 集成**：内嵌 Tauri 窗口打开 dsh Web UI（自动携带 token 免认证），或外部浏览器 / 桌面快捷方式
- **系统托盘**：打开主窗口 / 启动 / 停止 / 重启 / 退出，可配置"退出时驻留 dsh"
- **全局日志**：dsh 与启动器日志统一落盘（按天轮转 + 切割 + 保留 30 天），前端实时流式展示

## 安装

从 [Releases](https://github.com/youridol/dsh-launcher/releases) 下载最新 `dsh-launcher_<version>_x64-setup.exe` 安装包（Windows x64，NSIS 简体中文安装器，免管理员）。

## 开发

```bash
npm install          # 安装依赖（注意本机 .npmrc 若含 omit=dev 需 --include=dev）
npm run tauri dev    # 开发模式（前端 + Rust 热重载）
npm run build        # 前端构建（tsc + vite build）
npm run tauri build  # 打包安装包（NSIS）
```

Rust 后端入口在 `src-tauri/src/`（`core/` 核心逻辑 + `commands/` Tauri IPC），前端在 `src/`。

## 持续集成与发布

仓库内置 GitHub Actions 自动流水线：

- **CI**（`.github/workflows/ci.yml`）：每次 push / PR 自动执行 tsc 类型检查、前端构建、cargo check、单元与离线集成测试
- **Release**（`.github/workflows/release.yml`）：每次 push 到 master 自动迭代 PATCH 版本、更新 CHANGELOG、构建 NSIS 安装包并发布 GitHub Release

版本迭代由 `scripts/bump-version.mjs` 同步五文件（package.json / package-lock.json / Cargo.toml / Cargo.lock / tauri.conf.json，见 ADR-0004），幂等防重复发布。

## 文档

- `CONTEXT.md`：术语表
- `docs/DESIGN.md`：设计总览
- `docs/adr/`：架构决策记录（ADR-0001~0004）

## 技术栈

Tauri 2 · Rust · React 19 · TypeScript · Vite · Tailwind CSS 4 · shadcn/ui

## 许可

[MIT](LICENSE) © 2026 dsh-launcher contributors
