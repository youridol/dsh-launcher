# ADR-0004 — 启动器版本号策略：SemVer 风格递进 + 五处同步

- **状态**：Accepted（2026-09-03 修订；原"十进一"方案已被实际脚本取代，见"修订说明"）
- **日期**：2026-08-29（2026-09-03 修订）

## 决策

- 启动器版本独立于 dsh 版本（语义化版本 SemVer 风格）。
- 递进规则由 `scripts/bump-version.mjs` 统一实现：`--patch`（默认）/ `--minor` / `--major`；
  `0.4.9 → 0.4.10 → 0.4.11` 即为 patch 递进（不采用旧文档所谓"十进一 0.1.9→0.2.0"）。
- **五处同步**：`package.json` + `package-lock.json` + `src-tauri/Cargo.toml` +
  `src-tauri/Cargo.lock` + `src-tauri/tauri.conf.json`（bump-version.mjs 校验五处一致后才递增）。
- `CHANGELOG.md` 顶部插中文条目（含日期）；发布流程把该版本条目写入 GitHub Release notes。
- 版本选择器（UI）中 dsh 版本与启动器版本分开展示。

## 理由

- 十进一滚动在跨 10 位（0.4.9→0.4.10）时会被误读为"进位"，维护与发布自动化更易错；
  采用 SemVer 风格后可由脚本确定性递增，且与 CI 的 `--patch/--minor/--major` 手工触发语义一致。
- dsh 通道版本（如 `dsh-v0.1.2-alpha.1`）属于 dsh 自身版本体系，启动器版本与其彻底解耦。

## 修订说明

- 2026-08-29 初稿："非标准十进一递进（0.1.9→0.2.0）" + 三处同步。
- 2026-09-03：代码事实核对发现 bump-version.mjs 实现为 SemVer 风格且同步五处（含两个 lock 文件）；
  为消除"文档与实现矛盾"（审计 L8/L5），以本修订版为准。实现若有变更请同步修订本文档。
