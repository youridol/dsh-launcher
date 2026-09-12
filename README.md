# dsh-launcher

[deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) 的 Windows 桌面启动器与运行环境管理器（Tauri 2 + React + TypeScript + shadcn/ui）。

## 功能

- **dsh 生命周期管理**：启动 / 停止 / 重启 `dsh web`（端口可配置，默认 3080），状态实时探测
- **双通道版本管理**：npm 通道（registry 装包）与 GitHub 通道（clone + pnpm 构建），最新版本置顶，安装进度实时展示
- **工具链一键安装**：检测 Node / npm / pnpm / Git / Python，支持镜像源（npm registry / GitHub 加速 / Node 二进制）
- **Web GUI 集成**：内嵌 Tauri 窗口打开 dsh Web UI（自动携带 token 免认证），或外部浏览器 / 桌面快捷方式
- **系统托盘**：打开主窗口 / 启动 / 停止 / 重启 / 退出，可配置"退出时驻留 dsh"
- **全局日志**：dsh 与启动器日志统一落盘（按天轮转 + 切割 + 保留 30 天），前端实时流式展示
- **插件管理**（ADR-0005）：按包名独立启停（写 profile 受管 patch 区块，dsh 热重载，无需重启）、
  装卸（官方 `dsh plugin` 通道，自动重启）、upstream 自动同步（npm 版本 / git commit，git 钉 sha），
  自研插件（本地路径）不参与同步
- **技能共享**（ADR-0005）：以 `~/.agents/agent` 为唯一真源，把 `skills/`、`AGENTS.md`、`CONTEXT.md`
  共享给 `~/.dsh`（链接模式；无符号链接权限时降级为 home 层配置模式）
- **命令行**：`dsh-launcher plugin|skill ...`（与 GUI 共用同一套逻辑，便于脚本化/CI 验收）

## 安装

从 [Releases](https://github.com/youridol/dsh-launcher/releases) 下载最新 `dsh-launcher_<version>_x64-setup.exe` 安装包（Windows x64，NSIS 简体中文安装器，免管理员）。

> **关于 Windows SmartScreen 提示**：发布产物**未做代码签名**（这是刻意的产品决策，非构建疏忽）。
> 首次运行可能弹出「Windows 已保护你的电脑 / 未知发布者」——请点「更多信息」→「仍要运行」。
> 若需核对来源，请比对下载页附带的 SHA-256 校验值，或直接从本仓库的
> [Releases](https://github.com/youridol/dsh-launcher/releases)（由 CI 构建上传）获取。
> 若未来改变分发策略，可参考 `docs/AUDIT_REPORT_STRICT.md` 的 ENG-05 备查配置接入签名。

## 开发

```bash
npm install          # 安装依赖（注意本机 .npmrc 若含 omit=dev 需 --include=dev）
npm run tauri dev    # 开发模式（前端 + Rust 热重载）
npm run build        # 前端构建（tsc + vite build）
npm run tauri build  # 打包安装包（NSIS）—— ★ 发布/本地产出可运行 exe 的唯一推荐方式
```

### ⚠️ 发布构建必须启用 `custom-protocol`

**不要用裸 `cargo build --release` 产出发布件。** 原因：`tauri::is_dev()` 的实现是
`!cfg!(feature = "custom-protocol")`（`tauri-2.11.5/src/lib.rs:308`）。缺少该 feature 时，
即使加了 `--release`，二进制仍处于**开发模式**：启动后去加载 `build.devUrl`
（`http://localhost:1420`）而不是内嵌的前端资源 → 窗口报 `ERR_CONNECTION_REFUSED`
（典型现象：“localhost 拒绝连接”），且构建输出目录**不会**生成 `tauri-codegen-assets`。

- ✅ `npm run tauri build` —— `tauri build` 会自动启用 `custom-protocol`（CI 的
  tauri-action 同理），产出 `src-tauri/target/release/dsh-launcher.exe` + NSIS 安装包。
- ✅ `cargo build --release --features custom-protocol` —— 仅当需要直调 cargo 时。
- ❌ `cargo build --release` —— 不启用任何非 default feature，会得到 dev 模式二进制。

> 自检：若不确定产物是否正常，检查构建输出目录是否出现
> `target/release/build/dsh-launcher-*/out/tauri-codegen-assets/`（有 = 前端已内嵌）。

Rust 后端入口在 `src-tauri/src/`（`core/` 核心逻辑 + `commands/` Tauri IPC + `cli.rs` 无 GUI CLI），前端在 `src/`。

命令行（无需 GUI，适合脚本化验收）：

```bash
dsh-launcher plugin list [--json]
dsh-launcher plugin enable|disable|uninstall <package>
dsh-launcher plugin install <spec> [--origin upstream|in-house]
dsh-launcher plugin sync [--check]        # 同步 upstream 插件（--check 只检查）
dsh-launcher skill status [--json]
dsh-launcher skill apply [--mode auto|link|config]
dsh-launcher skill migrate [--dry-run]    # 迁移冲突资源（原文件改名保留，绝不删除）
```

## 持续集成与发布

仓库内置 GitHub Actions 自动流水线：

- **CI**（`.github/workflows/ci.yml`）：每次 push / PR 自动执行 tsc 类型检查、前端构建、cargo check、单元与离线集成测试
- **Release**（`.github/workflows/release.yml`）：每次 push 到 master 自动迭代 PATCH 版本、更新 CHANGELOG、构建 NSIS 安装包并发布 GitHub Release

版本迭代由 `scripts/bump-version.mjs` 同步五文件（package.json / package-lock.json / Cargo.toml / Cargo.lock / tauri.conf.json，见 ADR-0004），幂等防重复发布。

## 文档

- `CONTEXT.md`：术语表
- `docs/DESIGN.md`：设计总览
- `docs/adr/`：架构决策记录（ADR-0001~0005）

## 技术栈

Tauri 2 · Rust · React 19 · TypeScript · Vite · Tailwind CSS 4 · shadcn/ui

## 许可

[MIT](LICENSE) © 2026 dsh-launcher contributors
