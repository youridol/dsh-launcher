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
use std::io::{BufRead, BufReader};
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
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
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

        // 启动命令：优先 PATH 中的 dsh（npm 全局）；
        // 找不到则用 GitHub 安装目录内的 `pnpm dsh web`（cwd=安装目录），
        // 修复 v0.2.3：GitHub 安装后 dsh 不在 PATH → program not found。
        let mut cmd = command::hidden("dsh");
        cmd.args(["web", "--port", &port.to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        // 探测 PATH 中的 dsh 是否存在；不存在则改用安装目录内 pnpm dsh
        let dsh_in_path = command::hidden("dsh")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        let github_dir = crate::core::github::github_clone_dir();
        if !dsh_in_path && github_dir.join("package.json").exists() {
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Info,
                &format!("PATH 中无 dsh，改用安装目录启动: {}", github_dir.display()),
            );
            // pnpm dsh web --port <p>，cwd = 安装目录（pnpm 脚本: node --import tsx/esm apps/cli/src/bin.ts）
            let mut c = command::hidden_cmd("pnpm");
            c.args(["dsh", "web", "--port", &port.to_string()])
                .current_dir(&github_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null());
            cmd = c;
        } else if dsh_in_path {
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Info,
                "使用 PATH 中的 dsh 启动",
            );
        } else {
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Error,
                "PATH 中无 dsh 且未找到 GitHub 安装目录",
            );
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
        let pid = self.pid.lock().unwrap().clone();

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
        let mut pid = self.pid.lock().unwrap().clone();
        if pid == 0 {
            let port = self.port.lock().unwrap().clone();
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
        let mut sigterm = command::hidden("taskkill");
        let sigterm = sigterm
            .args(["/PID", &pid.to_string(), "/T"])
            .output();

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
        self.stop_locked()?;
        // 等待端口释放
        thread::sleep(Duration::from_millis(500));
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
            Ok(out) => String::from_utf8_lossy(&out.stdout).contains("dshmarket"),
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

    /// 后台监视线程：读输出写日志 + 进程退出时更新状态
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
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            // 两个独立线程并行读取 stdout/stderr，避免管道互堵：
            // 原实现"先读完全部 stdout 再读 stderr"，若 stdout 长期无数据且进程存活，
            // stderr 输出永远不会被读取 → 日志流缺失（v0.1.7 修复）。
            let logger_out = Arc::clone(&logger);
            let logger_err = Arc::clone(&logger);
            let t_out = stdout.map(|pipe| {
                thread::spawn(move || {
                    let reader = BufReader::new(pipe);
                    for line in reader.lines().map_while(Result::ok) {
                        let line = line.trim_end_matches('\r').to_string();
                        // 捕获 dsh web 完整 URL（含 token），供内嵌/外部打开免认证
                        if let Some(url) = extract_web_url(&line) {
                            if let Ok(mut u) = self_url.lock() {
                                *u = url;
                            }
                        }
                        logger_out.log(LogSource::Dsh, LogLevel::Info, &line);
                    }
                })
            });
            let t_err = stderr.map(|pipe| {
                thread::spawn(move || {
                    let reader = BufReader::new(pipe);
                    for line in reader.lines().map_while(Result::ok) {
                        let line = line.trim_end_matches('\r').to_string();
                        logger_err.log(LogSource::Dsh, LogLevel::Warn, &line);
                    }
                })
            });
            // 等待两个读取线程收尾（stdout/stderr 管道 EOF 后返回）
            if let Some(t) = t_out {
                let _ = t.join();
            }
            if let Some(t) = t_err {
                let _ = t.join();
            }

            // 进程退出后：回收子进程
            let wait_res = child.wait();
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

/// 解码 Windows 控制台输出（GBK/GB18030 编码；UTF-8 优先，失败回退 GBK）
/// 修复 taskkill 等系统命令错误信息乱码（中文 Windows 输出 GBK）
fn decode_console_text(bytes: &[u8]) -> String {
    // 优先按 UTF-8 解码（正常 Unicode 程序输出）
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    // 失败 → 按 GBK（Windows 中文代码页 936）解码
    let (cow, _, _) = encoding_rs::GBK.decode(bytes);
    cow.into_owned()
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
            let text = String::from_utf8_lossy(&out.stdout);
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
