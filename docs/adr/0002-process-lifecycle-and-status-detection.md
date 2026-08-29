# ADR-0002 — 进程生命周期与运行状态检测：信号驱动 + 端口探活

- **状态**：Accepted
- **日期**：2026-08-29
- **背景**：需求要求 dsh 生命周期控制（启动/停止/重启）、自动检测运行状态，并注明"用 dsh 官方提供接口"。事实核查：**dsh 没有官方 health/status/stop 命令或 HTTP 状态接口**。CLI reference 明确：收到 `SIGINT`/`SIGTERM` 时挂载根节点先 dispose 再退出（优雅排空 ≤ 5 秒），第二次信号立即强制退出。

## 决策

- **启动**：spawn `dsh web --port <p>` 子进程，记录 PID，注入工具链 PATH，CWD = 运行目录（workspace root）。
- **停止**：向 PID 发 `SIGTERM`（Windows 上等价于 `taskkill /PID <pid> /T`），等待 ≤ 5 秒优雅退出，超时再发一次强杀。**不发明 stop 命令**。
- **重启**：停止后重新启动（可能换端口）。
- **运行状态检测**：`port probe`（TCP 连接目标端口，成功即认为 dsh 在服务）+ `process match`（命令行含 `dsh web` / `@deepseek-ai/dsh` 的进程）。二者任一命中即 `running`。
  - **兜底**：用户手动在终端启动的 dsh 也要能被检测到 → 端口探测是主判据，PID 记录是辅助（区分"我管的"与"别人起的"）。
- **DSH_HOME**：只读展示，不接管。

## 理由

- 用户要求"官方接口"，但官方接口是**信号协议**（SIGTERM 优雅排空、二次信号强退）而非查询接口；必须如实告知偏差。
- 端口探活能覆盖"用户手动启动的 dsh"，满足 Q6 的明确要求。
- 信号优雅排空保证插件/会话数据安全，优于直接 kill。

## 后果

- Rust 后端持有进程句柄与 PID 映射；端口探测需处理端口被非 dsh 进程占用的情况（归入端口冲突处理）。
- Windows 下信号模拟用 `taskkill`，需验证优雅排空路径（dsh 是 Node 进程，SIGTERM 语义由 Node 运行时承担）。
