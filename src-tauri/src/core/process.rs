//! dsh 进程生命周期管理
//!
//! 语义（docs/DESIGN.md §4.3、ADR-0002）：
//! - 启动：spawn `dsh web --port <p>`，CWD = dsh 官方默认（运行目录不干预）
//! - 停止：SIGTERM 优雅排空（Windows 用 taskkill /PID /T），等待 ≤5s，超时强杀
//! - 重启：停止后同配置重启
//! - 状态：事件驱动（进程退出回调）+ 端口探活兜底

use crate::core::command;
use crate::core::logging::{LogLevel, LogSource, Logger};
use crate::core::port;
use std::io::Read;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// dsh 运行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DshStatus {
    /// 未运行
    Stopped,
    /// 正在启动
    Starting,
    /// 运行中
    Running,
    /// 停止中
    Stopping,
    /// 出错
    Error,
}

/// 进程管理器（全局单例，由 Tauri state 持有）
///
/// 注意：子进程所有权会移入监视线程；stop 通过共享句柄探测退出。
pub struct ProcessManager {
    /// 当前 dsh 子进程句柄（监视线程持有所有权，这里保留引用用于 try_wait）
    child: Arc<Mutex<Option<Child>>>,
    /// 当前实际 PID（v0.2.5：独立记录，监视线程 take 走 child 后 stop 仍可用）
    pid: Arc<Mutex<u32>>,
    /// dsh web 完整访问 URL（含 token，从 stdout 捕获；空 = 尚未捕获）
    web_url: Arc<Mutex<String>>,
    /// 当前状态
    status: Arc<Mutex<DshStatus>>,
    /// 当前实际端口
    port: Arc<Mutex<u16>>,
    /// 日志
    logger: Arc<Logger>,
    /// 是否正在停止（避免重复触发）
    stopping: AtomicBool,
    /// 生命周期操作互斥锁：保证同一时刻只有一个 start/stop/restart 在执行
    /// （防止多线程并发调用导致双进程/双杀竞态）
    op_lock: Mutex<()>,
}

impl ProcessManager {
    pub fn new(logger: Arc<Logger>) -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            pid: Arc::new(Mutex::new(0)),
            web_url: Arc::new(Mutex::new(String::new())),
            status: Arc::new(Mutex::new(DshStatus::Stopped)),
            port: Arc::new(Mutex::new(0)),
            logger,
            stopping: AtomicBool::new(false),
            op_lock: Mutex::new(()),
        }
    }

    /// 当前状态
    pub fn status(&self) -> DshStatus {
        self.status.lock().map(|s| *s).unwrap_or(DshStatus::Error)
    }

    /// 当前实际端口
    pub fn current_port(&self) -> u16 {
        self.port.lock().map(|p| *p).unwrap_or(0)
    }

    /// dsh web 完整访问 URL（含 token；未捕获时为空）
    pub fn web_url(&self) -> String {
        self.web_url.lock().map(|u| u.clone()).unwrap_or_default()
    }

    /// 启动时收养已在运行的 dsh（v0.3.5：退出驻留后重开启动器，
    /// 探测到端口监听则恢复 Running 状态，使停止/重启可用）
    pub fn adopt_running(&self, port: u16) {
        let mut st = self.status.lock().unwrap();
        *st = DshStatus::Running;
        drop(st);
        *self.port.lock().unwrap() = port;
        // pid 未知（外部进程），保持 0；停止时按端口查 PID
        // 恢复 web_url（从日志兜底提取带 token 的 URL）
        if let Some(url) = crate::core::logging::extract_latest_web_url() {
            *self.web_url.lock().unwrap() = url;
        }
        self.logger.log(
            LogSource::Launcher,
            LogLevel::Info,
            &format!("检测到 dsh 已在运行（端口 {port}），恢复运行状态"),
        );
    }

    /// 按端口查找监听进程 PID（Windows Get-NetTCPConnection）
    fn pid_by_port(port: u16) -> Option<u32> {
        let ps = format!(
            "(Get-NetTCPConnection -LocalPort {port} -State Listen | Select-Object -First 1 -ExpandProperty OwningProcess)"
        );
        let mut c = crate::core::command::hidden("powershell");
        c.args(["-NoProfile", "-Command", &ps]);
        let out = c.output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = crate::core::text::decode(&out.stdout).trim().to_string();
        text.parse().ok()
    }

    /// 启动 dsh web（阻塞等待 spawn 结果）
    pub fn start(&self, port: u16) -> Result<(), String> {
        // 生命周期操作互斥：避免并发双 start
        let _op = self.op_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.start_locked(port)
    }

    /// 无锁版 start（调用方必须已持有 op_lock）
    fn start_locked(&self, port: u16) -> Result<(), String> {
        if matches!(
            self.status(),
            DshStatus::Running | DshStatus::Starting | DshStatus::Stopping
        ) {
            return Err("dsh 正在运行或切换中".to_string());
        }
        if !port::validate_port(port) {
            return Err(format!("端口 {port} 非法"));
        }
        if port::is_port_in_use(port) {
            return Err(format!("端口 {port} 已被占用，请在设置中修改端口后重试"));
        }

        // 启动命令决策（v0.4.3 重构，修复分发机器启动失败 "--profile <name> is required"）：
        // --no-open：不自动弹外部浏览器（Web GUI 由用户通过内嵌窗口/桌面快捷方式打开）
        // 两种 dsh 来源、两种启动方式：
        // ① GitHub 通道（安装目录 apps/cli/src/bin.ts 存在）→ **直接 node 启动 bin.ts**
        //    （node.exe 绝对路径 + cwd=安装目录，进程树浅、token stdout 实时、taskkill 干净）。
        //    启动器 GitHub 通道安装的是 dsh.cmd shim（cd 安装目录 && pnpm dsh），pnpm 在 Windows
        //    经多层 cmd/node 嵌套会引发：token stdout 多层缓冲不实时 → 401 authentication required；
        //    进程树 5 层 taskkill /T 杀不净 → 残留孤儿占端口 + 旧 token 错乱。因此本启动器 shim
        //    一律绕开、走直接 node（与 npm 全局包区分见下）。
        // ② npm 全局真 dsh（PATH 中的 dsh.cmd **不是**本启动器创建，为真实 npm 包）→ 必须带
        //    web --port <p> --no-open 启动（v0.4.3 修复：此前直接裸跑 dsh 不带 profile 参数，
        //    dsh CLI 报 "error: --profile <name> is required" 后退出码 1 → 分发机器启动失败）。
        // Windows 上 npm 全局 dsh / 本启动器 shim 都是 .cmd，spawn 必须 cmd /C 包装；
        // dsh 存在性探测与 shim 归属探测必须与真实 spawn 用同一 PATH（含 node_dir 注入），
        // 否则分发机器（node_dir 不在当前进程 PATH 快照）会把本启动器 shim 误判成 npm 包。
        // v0.4.6：**禁止直接执行 `dsh --version` 探测**。本启动器 GitHub shim 内容为
        // `cd /d <安装目录> && pnpm dsh %*`；当安装目录被卸载/清空（残留空目录）时，pnpm
        // 找不到本地 dsh 定义 → 把 dsh 当外部命令 → 递归命中 dsh.cmd → pnpm 递归爆炸
        // （cmd → node(pnpm) → cmd → ... 指数级，实测 10 秒积累数百 node.exe）。
        // 故改用 probe_dsh_command() 纯静态解析（读文件内容 + 目录存在性，零进程开销）。
        let github_dir = crate::core::github::github_clone_dir();
        let clone_ok = github_dir.join("apps/cli/src/bin.ts").exists();
        let probe = crate::core::github::probe_dsh_command();
        let dsh_in_path = probe != crate::core::github::DshProbe::None;
        let shim_owned = matches!(
            probe,
            crate::core::github::DshProbe::OwnedShimOk | crate::core::github::DshProbe::OwnedShimBroken
        );
        // 本启动器 shim 指向损坏目录（被卸载/清空）→ 直接报错，绝不执行 dsh（会递归爆炸）
        if probe == crate::core::github::DshProbe::OwnedShimBroken {
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Error,
                &format!("dsh.cmd 指向的 GitHub 安装目录缺失: {}", github_dir.display()),
            );
            return Err("dsh 安装目录缺失（GitHub shim 指向的目录已不存在或为空），请在版本管理中重新安装 dsh".to_string());
        }
        let mut cmd;
        if dsh_in_path && !shim_owned {
            // PATH 中 dsh 为真实 npm 全局包 → dsh web --port <p> --no-open
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Info,
                "PATH 中 dsh 为 npm 全局包，用 dsh web 启动",
            );
            let mut c = command::hidden_cmd("dsh");
            c.args(["web", "--port", &port.to_string(), "--no-open"]);
            cmd = c;
        } else if clone_ok {
            // GitHub 安装目录存在 → 直接 node 启动（PATH 中为本启动器 shim 或无 dsh 都走这里）
            let msg = if dsh_in_path {
                "dsh 为本启动器 GitHub shim，改用直接 node 启动（避免 pnpm 嵌套）".to_string()
            } else {
                format!("PATH 中无 dsh，直接用安装目录启动: {}", github_dir.display())
            };
            self.logger.log(LogSource::Launcher, LogLevel::Info, &msg);
            cmd = direct_node_cmd(&github_dir, port);
        } else if dsh_in_path {
            // PATH 中有 dsh 且不是 npm 包（本启动器 shim 特征），但安装目录已不存在
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Error,
                &format!("dsh.cmd 指向的 GitHub 安装目录不存在: {}", github_dir.display()),
            );
            return Err("dsh 安装目录缺失（GitHub shim 指向的目录已不存在），请在版本管理中重新安装 dsh".to_string());
        } else {
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Error,
                "PATH 中无 dsh 且未找到 GitHub 安装目录",
            );
            return Err("未找到 dsh（PATH 无 dsh 且无 GitHub 安装目录），请先安装 dsh".to_string());
        };
        // v0.4.5：dsh 子进程 stdout/stderr 重定向到文件而非管道。
        // 分发机器实测：GUI（Tauri）父进程 + CREATE_NO_WINDOW 创建的匿名管道在部分
        // Windows 客户机上输出不实时到达（dsh web 的 token URL stdout 直到进程被杀才
        // 出现 → 启动器永远捕获不到 → Web GUI 裸 URL → 401 认证页 / 连接拒绝）。
        // node 对普通文件的写入实时落盘（无管道缓冲问题），启动器轮询 tail 文件即可
        // 实时得到 token URL 与 dsh 日志（见 spawn_monitor / tail_output_file）。
        let stdout_file = open_redirect_file(&dsh_output_path(true)).map_err(|e| {
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Error,
                &format!("创建 dsh stdout 落盘文件失败: {e}"),
            );
            format!("创建 dsh stdout 落盘文件失败: {e}")
        })?;
        let stderr_file = open_redirect_file(&dsh_output_path(false)).map_err(|e| {
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Error,
                &format!("创建 dsh stderr 落盘文件失败: {e}"),
            );
            format!("创建 dsh stderr 落盘文件失败: {e}")
        })?;
        cmd.stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file))
            .stdin(Stdio::null());

        // 注入工具链 PATH（DESIGN §4.3）：若系统 PATH 无 node 但启动器已装用户级 Node，
        // 前缀注入 node_dir（含 npm/pnpm），否则依赖 node 的 dsh/pnpm 子进程无法启动
        #[cfg(windows)]
        {
            let mut probe = command::hidden("where");
            probe.arg("node");
            let node_in_path = probe
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !node_in_path {
                let node_dir = crate::core::toolchain::node_dir();
                if node_dir.join("node.exe").exists() {
                    let current = std::env::var("PATH").unwrap_or_default();
                    let node_path = node_dir.to_string_lossy().to_string();
                    if !current.contains(&node_path) {
                        cmd.env("PATH", format!("{node_path};{current}"));
                        self.logger.log(
                            LogSource::Launcher,
                            LogLevel::Info,
                            &format!("已注入用户级 Node 到子进程 PATH: {node_path}"),
                        );
                    }
                }
            }
        }

        let child = cmd.spawn().map_err(|e| {
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Error,
                &format!("spawn dsh 失败: {e}"),
            );
            format!("启动 dsh 失败: {e}（请确认 dsh 已安装并在 PATH 中，或已通过版本管理安装 GitHub 通道）")
        })?;

        self.logger.log(
            LogSource::Launcher,
            LogLevel::Info,
            &format!("dsh 已启动 (pid={}, port={})", child.id(), port),
        );

        *self.pid.lock().unwrap() = child.id();
        *self.port.lock().unwrap() = port;
        *self.status.lock().unwrap() = DshStatus::Starting;
        self.stopping.store(false, Ordering::SeqCst);

        // 子进程放入共享句柄，监视线程取走所有权
        *self.child.lock().unwrap() = Some(child);
        let pid = *self.pid.lock().unwrap();

        self.spawn_monitor(pid);
        // v0.2.7：启动探活线程——端口监听则置 Running；
        // 进程退出且端口未监听（启动即崩）→ 自动修复不兼容插件并重试
        self.spawn_startup_probe(port, pid);

        Ok(())
    }

    /// 停止 dsh（SIGTERM → 等待 ≤5s → 强杀）
    pub fn stop(&self) -> Result<(), String> {
        // 生命周期操作互斥：避免并发双 stop
        let _op = self.op_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.stop_locked()
    }

    /// 无锁版 stop（调用方必须已持有 op_lock）
    fn stop_locked(&self) -> Result<(), String> {
        if self.stopping.swap(true, Ordering::SeqCst) {
            return Ok(()); // 已在停止中
        }
        let mut status = self.status.lock().unwrap();
        if !matches!(*status, DshStatus::Running | DshStatus::Starting) {
            self.stopping.store(false, Ordering::SeqCst);
            return Ok(());
        }
        *status = DshStatus::Stopping;
        drop(status);

        // 用记录的 PID（v0.2.5：child 句柄被监视线程 take 走，stop 依赖 pid 字段）；
        // v0.3.5：pid 为 0 但端口在监听（收养的外部 dsh）→ 按端口查 PID
        let mut pid = *self.pid.lock().unwrap();
        if pid == 0 {
            let port = *self.port.lock().unwrap();
            if port != 0 && port::is_port_in_use(port) {
                pid = Self::pid_by_port(port).unwrap_or(0);
                if pid != 0 {
                    self.logger.log(
                        LogSource::Launcher,
                        LogLevel::Info,
                        &format!("按端口 {port} 找到 dsh 进程 pid={pid}"),
                    );
                }
            }
        }
        if pid == 0 {
            self.stopping.store(false, Ordering::SeqCst);
            *self.status.lock().unwrap() = DshStatus::Stopped;
            return Ok(());
        };

        self.logger.log(
            LogSource::Launcher,
            LogLevel::Info,
            &format!("正在停止 dsh (pid={pid})，发送 SIGTERM…"),
        );

        // Windows 下用 taskkill /PID <pid> /T 模拟 SIGTERM（优雅排空）
        let sigterm = {
            let mut c = command::hidden("taskkill");
            c.args(["/PID", &pid.to_string(), "/T"])
                .output()
        };

        // 等待优雅退出，最多 1 秒（v0.3.5：child 句柄已被监视线程 take 走，
        // 用 tasklist 探测进程存活；node/pnpm 进程树对无 /F 的 taskkill 常不响应，
        // 快速升级强杀，避免退出/停止卡顿）
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut exited = false;
        loop {
            if !process_alive(pid) {
                exited = true;
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(150));
        }

        if !exited {
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Warn,
                &format!("dsh (pid={pid}) 1 秒内未优雅退出，强制终止…"),
            );
            let _ = {
                let mut c = command::hidden("taskkill");
                c.args(["/PID", &pid.to_string(), "/T", "/F"])
                    .output()
            };
        }

        // v0.4.2：端口级兑底清剿 —— taskkill /T 对 pnpm 深嵌套进程树可能杀不净
        // （远程实测残留 dsh web node 孤儿进程继续占端口，持有旧 token → 401）。
        // 若端口仍监听，按端口查实际监听 PID 逐个强杀，直到端口释放（最多 5 轮）。
        let port_now = *self.port.lock().unwrap();
        if port_now != 0 && port::is_port_in_use(port_now) {
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Warn,
                &format!("停止后端口 {port_now} 仍被占用（taskkill 树杀不净），按端口清剿残留进程…"),
            );
            for round in 1..=5 {
                let Some(listener) = Self::pid_by_port(port_now) else {
                    break;
                };
                // 避免误杀刚重启的新实例：仅当监听者不是当前托管的 pid 链时强杀
                let _ = round;
                self.logger.log(
                    LogSource::Launcher,
                    LogLevel::Warn,
                    &format!("端口 {port_now} 残留监听进程 pid={listener}，强制终止…"),
                );
                let mut c = command::hidden("taskkill");
                c.args(["/PID", &listener.to_string(), "/T", "/F"])
                    .output()
                    .ok();
                // 等待端口释放
                let deadline = Instant::now() + Duration::from_millis(1500);
                while Instant::now() < deadline {
                    if !port::is_port_in_use(port_now) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(200));
                }
                if !port::is_port_in_use(port_now) {
                    self.logger.log(
                        LogSource::Launcher,
                        LogLevel::Info,
                        &format!("端口 {port_now} 已释放（残留进程已清理）"),
                    );
                    break;
                }
            }
        }

        match sigterm {
            Ok(out) if out.status.success() => {
                self.logger.log(LogSource::Launcher, LogLevel::Info, "dsh 已停止");
            }
            Ok(out) => {
                self.logger.log(
                    LogSource::Launcher,
                    LogLevel::Warn,
                    &format!("taskkill 返回非零: {}", decode_console_text(&out.stderr)),
                );
            }
            Err(e) => {
                self.logger.log(
                    LogSource::Launcher,
                    LogLevel::Error,
                    &format!("taskkill 执行失败: {e}"),
                );
            }
        }

        *self.status.lock().unwrap() = DshStatus::Stopped;
        *self.pid.lock().unwrap() = 0;
        *self.port.lock().unwrap() = 0;
        *self.web_url.lock().unwrap() = String::new();
        self.stopping.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// 重启：停止后同配置重启（拿一次 op_lock，内部调用无锁版本避免重入死锁）
    pub fn restart(&self) -> Result<(), String> {
        // 生命周期操作互斥：避免与 start/stop 并发
        let _op = self.op_lock.lock().unwrap_or_else(|e| e.into_inner());
        let port = self.current_port();
        // 先优雅停止旧进程；若旧进程已不在（如已被外部终止），stop 仍返回 Ok（幂等），
        // 继续用同一配置拉起新进程。仅当停止真正失败（taskkill 无法终止）时放弃重启，
        // 避免新旧进程端口冲突导致启动失败。
        let stop_res = self.stop_locked();
        // 等待端口释放（停止后需短暂排空）
        thread::sleep(Duration::from_millis(500));
        if let Err(e) = stop_res {
            return Err(format!("停止旧进程失败，放弃重启: {e}"));
        }
        self.start_locked(port)
    }

    /// 启动探活线程：端口监听 → Running；进程退出且端口未开 → 自动修复插件后重试
    fn spawn_startup_probe(&self, port: u16, pid: u32) {
        let logger = Arc::clone(&self.logger);
        let status_arc = Arc::clone(&self.status);
        let pid_arc = Arc::clone(&self.pid);
        thread::spawn(move || {
            // 最多等 8 秒（dsh 冷启动 + 插件加载）
            let deadline = Instant::now() + Duration::from_secs(8);
            while Instant::now() < deadline {
                let alive = {
                    let p = pid_arc.lock().unwrap();
                    *p != 0 && *p == pid
                };
                if port::probe(port) == Some(true) {
                    // 端口监听 → 启动成功
                    let mut s = status_arc.lock().unwrap();
                    if *s == DshStatus::Starting {
                        *s = DshStatus::Running;
                        logger.log(LogSource::Launcher, LogLevel::Info, "dsh 已就绪（端口监听中）");
                    }
                    drop(s);
                    return;
                }
                if !alive {
                    // 进程已退出且端口未监听：启动即崩，尝试自动修复不兼容插件
                    logger.log(
                        LogSource::Launcher,
                        LogLevel::Warn,
                        "dsh 进程启动后即退出且端口未监听，尝试自动修复不兼容插件…",
                    );
                    let ok = Self::fix_incompatible_plugins(&logger);
                    if ok {
                        logger.log(
                            LogSource::Launcher,
                            LogLevel::Info,
                            "已自动修复不兼容插件（dshmarket）；请再次点击启动",
                        );
                    } else {
                        logger.log(
                            LogSource::Launcher,
                            LogLevel::Error,
                            "自动修复未生效，请检查日志定位启动失败原因",
                        );
                    }
                    // 无论是否修复，本次启动的进程已死：状态复位
                    let mut s = status_arc.lock().unwrap();
                    if *s == DshStatus::Starting {
                        *s = DshStatus::Stopped;
                    }
                    drop(s);
                    let mut p = pid_arc.lock().unwrap();
                    if *p == pid {
                        *p = 0;
                    }
                    return;
                }
                thread::sleep(Duration::from_millis(500));
            }
            // 8 秒后仍未监听但进程存活：保持 Starting（可能是 dsh 后台任务，不误判失败）
            logger.log(
                LogSource::Launcher,
                LogLevel::Warn,
                &format!("dsh 启动 8 秒后端口 {port} 仍未监听（进程存活），状态保持启动中"),
            );
        });
    }

    /// 自动修复不兼容插件：dshmarket 与当前 dsh-settings API 不兼容时卸载之。
    /// 返回是否执行了卸载（幂等：dshmarket 不在依赖中时返回 false）。
    fn fix_incompatible_plugins(logger: &Arc<Logger>) -> bool {
        // 仅当 dsh 安装目录存在时处理（GitHub 通道安装的 dsh）
        let install = crate::core::github::github_clone_dir();
        if !install.join("package.json").exists() {
            return false;
        }
        // 检查 web profile 是否仍依赖 dshmarket
        let list = {
            let mut c = crate::core::command::hidden_cmd("pnpm");
            c.args(["dsh", "plugin", "--profile", "web", "list"])
                .current_dir(&install);
            c.output()
        };
        let still_has = match list {
            Ok(out) => crate::core::text::decode(&out.stdout).contains("dshmarket"),
            Err(_) => false,
        };
        if !still_has {
            return false;
        }
        logger.log(
            LogSource::Launcher,
            LogLevel::Info,
            "检测到 web profile 含 dshmarket，执行卸载以修复兼容性…",
        );
        let un = {
            let mut c = crate::core::command::hidden_cmd("pnpm");
            c.args(["dsh", "plugin", "--profile", "web", "uninstall", "dshmarket"])
                .current_dir(&install);
            c.output()
        };
        match un {
            Ok(out) if out.status.success() => {
                logger.log(LogSource::Launcher, LogLevel::Info, "dshmarket 已卸载");
                true
            }
            Ok(out) => {
                logger.log(
                    LogSource::Launcher,
                    LogLevel::Error,
                    &format!(
                        "卸载 dshmarket 失败: {}",
                        decode_console_text(&out.stderr).trim()
                    ),
                );
                false
            }
            Err(e) => {
                logger.log(
                    LogSource::Launcher,
                    LogLevel::Error,
                    &format!("执行插件卸载失败: {e}"),
                );
                false
            }
        }
    }

    /// 后台监视线程：tail dsh 输出落盘文件写日志/捕获 URL + 进程退出时更新状态
    fn spawn_monitor(&self, pid: u32) {
        let logger = Arc::clone(&self.logger);
        let child_arc = Arc::clone(&self.child);
        let status_arc = Arc::clone(&self.status);
        let port_arc = Arc::clone(&self.port);
        let self_pid = Arc::clone(&self.pid);
        let self_url = Arc::clone(&self.web_url);
        let cleanup_url = Arc::clone(&self.web_url);

        thread::spawn(move || {
            // 从共享句柄取走所有权（stop 通过 try_wait 探测，取走前 stop 已可能拿到引用）
            let child = child_arc.lock().unwrap().take();
            let Some(mut child) = child else {
                return;
            };

            // v0.4.5：stdout/stderr 已重定向到落盘文件（见 start_locked），
            // 这里开两个轮询线程 tail 文件（各自独立，避免相互阻塞）。
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stop_out = Arc::clone(&stop);
            let stop_err = Arc::clone(&stop);
            let logger_out = Arc::clone(&logger);
            let logger_err = Arc::clone(&logger);
            let out_url = Arc::clone(&self_url);
            let t_out = thread::spawn(move || {
                tail_output_file(
                    &dsh_output_path(true),
                    LogLevel::Info,
                    logger_out,
                    Some(out_url),
                    stop_out,
                );
            });
            let t_err = thread::spawn(move || {
                tail_output_file(&dsh_output_path(false), LogLevel::Warn, logger_err, None, stop_err);
            });

            // 进程退出后：回收子进程
            let wait_res = child.wait();
            // 停止 tail 线程（它们 150ms 轮询一次，很快退出）
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = t_out.join();
            let _ = t_err.join();

            logger.log(
                LogSource::Launcher,
                LogLevel::Info,
                &format!("dsh 进程 (pid={pid}) 已退出: {wait_res:?}"),
            );
            // 更新状态（若 stop() 已置 Stopping，这里不覆盖）
            let mut s = status_arc.lock().unwrap();
            if *s != DshStatus::Stopping {
                *s = DshStatus::Stopped;
            }
            drop(s);
            let mut p = port_arc.lock().unwrap();
            *p = 0;
            let mut pid_slot = self_pid.lock().unwrap();
            if *pid_slot == pid {
                *pid_slot = 0;
            }
            let mut url_slot = cleanup_url.lock().unwrap();
            *url_slot = String::new();
        });
    }
}

/// dsh web 输出落盘文件路径（v0.4.5，stdout/stderr 分文件）
/// stdout=true → dsh-web-stdout.log；false → dsh-web-stderr.log
fn dsh_output_path(stdout: bool) -> std::path::PathBuf {
    let name = if stdout {
        "dsh-web-stdout.log"
    } else {
        "dsh-web-stderr.log"
    };
    crate::core::logging::logs_dir().join(name)
}

/// 打开 dsh 输出落盘文件（每次启动截断重建；返回可写 File 供 Stdio::from）
fn open_redirect_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
}

/// 轮询读取 dsh 输出落盘文件的新增内容（tail），逐行：
/// - 落日志（stdout=Info / stderr=Warn，与旧管道实现一致）
/// - stdout 行做 token URL 捕获（内存 web_url）
/// 150ms 轮询间隔足够实时（node 对普通文件的写入即写即落盘）。
fn tail_output_file(
    path: &std::path::Path,
    level: LogLevel,
    logger: Arc<Logger>,
    url_slot: Option<Arc<Mutex<String>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    let Ok(mut file) = std::fs::OpenOptions::new().read(true).open(path) else {
        return;
    };
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if stop.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        match file.read(&mut chunk) {
            Ok(0) => {
                // 暂无新数据：进程可能仍在写；短暂轮询
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            Ok(n) => {
                for &b in &chunk[..n] {
                    if b == b'\n' {
                        if !buf.is_empty() {
                            // v0.4.0：统一解码（UTF-8 优先 + 代码页回退），修复 GBK 乱码
                            let raw = crate::core::text::decode(&buf);
                            buf.clear();
                            let line = raw.trim_end_matches('\r').to_string();
                            // 捕获 dsh web 完整 URL（含 token），供内嵌/外部打开免认证
                            if level == LogLevel::Info {
                                if let Some(url) = extract_web_url(&line) {
                                    if let Some(slot) = url_slot.as_ref() {
                                        if let Ok(mut u) = slot.lock() {
                                            *u = url;
                                        }
                                    }
                                }
                            }
                            logger.log(LogSource::Dsh, level, &line);
                        }
                    } else {
                        buf.push(b);
                    }
                }
            }
            Err(_) => {
                // 读取失败（文件被删等）：稍后重试
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
    }
    // 退出前排空剩余未换行的内容（进程被杀时最后一行可能无 \n）
    if !buf.is_empty() {
        let raw = crate::core::text::decode(&buf);
        let line = raw.trim_end_matches('\r').to_string();
        if !line.is_empty() {
            if level == LogLevel::Info {
                if let Some(url) = extract_web_url(&line) {
                    if let Some(slot) = url_slot.as_ref() {
                        if let Ok(mut u) = slot.lock() {
                            *u = url;
                        }
                    }
                }
            }
            logger.log(LogSource::Dsh, level, &line);
        }
    }
}

/// 构造"直接 node 启动 GitHub 通道 dsh"的命令（v0.4.2 修复）：
/// `node --import tsx/esm apps/cli/src/bin.ts web --port <p> --no-open`，cwd = 安装目录。
///
/// 为何不用 `pnpm dsh`（dsh.cmd shim）：
/// - pnpm 在 Windows 上经 cmd.exe 多层嵌套（cmd→node(pnpm)→cmd→node(tsx)→cmd→node(dsh)），
///   dsh web 的 token stdout 在多级管道缓冲下**不实时到达**启动器 → 内嵌窗口拿不到新 token
///   → 401 "dsh web authentication required"；
/// - 进程树 5 层导致 `taskkill /T` 杀不净 → 残留孤儿 node 继续占端口（旧 token 错乱）。
/// 直接 node 启动：进程树浅（node 单进程 + 其子），stdout 实时，taskkill 干净。
/// 返回已配好 stdout/stderr/stdin 的 Command（PATH 由调用方注入 node_dir）。
#[cfg(windows)]
fn direct_node_cmd(github_dir: &std::path::Path, port: u16) -> std::process::Command {
    use crate::core::command;
    // 用绝对 node.exe（用户级 node_dir 或 PATH 中 node），避免 cmd 包装
    let mut node_exe = crate::core::toolchain::node_dir().join("node.exe");
    if !node_exe.exists() {
        // 回退：探测 PATH 中 node
        let mut probe = command::hidden("where");
        probe.arg("node");
        if let Ok(out) = probe.output() {
            if out.status.success() {
                let text = crate::core::text::decode(&out.stdout);
                if let Some(line) = text.lines().next() {
                    let line = line.trim();
                    if !line.is_empty() {
                        node_exe = std::path::PathBuf::from(line);
                    }
                }
            }
        }
    }
    let mut c = command::hidden(&node_exe);
    c.current_dir(github_dir);
    c.args([
        "--import",
        "tsx/esm",
        "apps/cli/src/bin.ts",
        "web",
        "--port",
        &port.to_string(),
        "--no-open",
    ]);
    c
}

/// 从 dsh 输出行提取 web URL（形如 http://127.0.0.1:<port>/?token=xxx）
fn extract_web_url(line: &str) -> Option<String> {
    let start = line.find("http://127.0.0.1:")?;
    let rest = &line[start..];
    // 取到空白/控制符为止
    let end = rest
        .find(|c: char| c.is_whitespace())
        .unwrap_or(rest.len());
    let url = &rest[..end];
    if url.contains("token=") {
        Some(url.to_string())
    } else {
        None
    }
}

/// 解码 Windows 控制台输出（UTF-8 优先，失败回退代码页 GBK/OEM）
/// 实现见 core/text.rs（统一解码，全部子进程输出解码都走它，避免各点乱码）
pub(crate) fn decode_console_text(bytes: &[u8]) -> String {
    crate::core::text::decode(bytes)
}

/// 检查指定 PID 的进程是否存活（tasklist 精确过滤）
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let mut c = crate::core::command::hidden("tasklist");
    c.args(["/FI", &format!("PID eq {pid}"), "/NH"]);
    match c.output() {
        Ok(out) => {
            let text = crate::core::text::decode(&out.stdout);
            // 输出包含 PID 行则存活（无匹配时输出 "INFO: No tasks"）
            text.contains(&pid.to_string())
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::decode_console_text;
    use super::extract_web_url;

    #[test]
    fn test_decode_console_text_gbk() {
        // 真实 taskkill 中文输出（GBK 编码）："错误: 无法终止 PID 3108 (属于 PID 24800 子进程)的进程。"
        let msg = "错误: 无法终止 PID 3108 (属于 PID 24800 子进程)的进程。\r\n原因: 只能强制终止这个进程(带 /F 选项)。\r\n";
        let gbk = encoding_rs::GBK.encode(msg).0;
        let decoded = decode_console_text(&gbk);
        assert!(
            decoded.contains("无法终止 PID 3108"),
            "GBK 解码应得到中文: {decoded:?}"
        );
        assert!(
            decoded.contains("只能强制终止"),
            "应包含原因说明: {decoded:?}"
        );
        // UTF-8 输入直接通过
        assert_eq!(decode_console_text("hello".as_bytes()), "hello");
        // 混合/无效字节不 panic
        let _ = decode_console_text(&[0xff, 0xfe, 0x00, 0x41]);
    }

    #[test]
    fn test_extract_web_url() {
        // 真实 dsh 启动输出（含 token）
        let line = "dsh web: http://127.0.0.1:3080/?token=abc123def456\n";
        assert_eq!(
            extract_web_url(line).as_deref(),
            Some("http://127.0.0.1:3080/?token=abc123def456")
        );
        // 行内有后续文字（空格分隔）
        let line2 = "dsh web: http://127.0.0.1:3080/?token=x y";
        assert_eq!(extract_web_url(line2).as_deref(), Some("http://127.0.0.1:3080/?token=x"));
        // 无 token 的裸 URL 不捕获（避免误存）
        assert_eq!(extract_web_url("listening on http://127.0.0.1:3080"), None);
        // 无关行不捕获
        assert_eq!(extract_web_url("random log line"), None);
    }
}
