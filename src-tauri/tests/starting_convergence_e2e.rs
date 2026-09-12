//! v0.9.1 端到端回归：**dsh 冷启动超过探活线程 8 秒上限时，状态必须仍能收敛为 Running**
//!
//! ## 覆盖的缺陷（"启动后未就绪"BUG）
//!
//! 修复前：`spawn_startup_probe` 是唯一的 `Starting → Running` 路径，硬上限 8 秒；
//! dsh 冷启动一旦超过 8 秒，该线程打一条 warn 后 `return`，**再无任何后续收敛尝试**。
//! 后果：状态永久卡 `Starting` →
//!   - 前端「内嵌打开」读到的一直不是 `running`，25 秒后误报"端口未监听"；
//!   - UI 上 `Starting` 等同"运行中"（端口输入框被禁用）；
//!   - 收养分支只认 `Stopped`，故"手动启动的 dsh"也无法被接管。
//!
//! 本测试用**真实 dsh 冷启动**（其耗时由插件/MCP 决定，本机实测 ~8.8s 才监听端口）
//! 验证修复后的收敛链：`start()` → 端口监听 → `reconcile_now()` → `Running`。
//!
//! ## 为什么必须真启 dsh
//!
//! 缺陷的本质是**真实冷启动耗时 > 硬编码上限**这一时序事实；用 mock 替身无法复现
//! （mock 的"耗时"由测试自己编造，也就无法证明真实上限是否够用）。
//!
//! ## 隔离（重要）
//!
//! 真实拉起 dsh 会写 `%LOCALAPPDATA%\dsh-launcher\logs\dsh-web-{stdout,stderr}.log`
//! （**每次启动截断重建**）与 `last-web-url` 缓存；若不隔离，本测试会破坏用户**正在
//! 运行**的启动器会话的文件。故用例通过 `DSH_LAUNCHER_DATA_DIR` 指向各自的临时目录
//! （见 `core/logging.rs::data_dir`）。
//!
//! 因该环境变量是**进程级**的，两个用例必须串行执行：
//! `cargo test --test starting_convergence_e2e -- --ignored --test-threads=1`
//!
//! 前置：本机已安装 deepseek-harness 且 node 可用（缺失时断言明确失败并给出指引）。

use dsh_launcher_lib::core::logging::Logger;
use dsh_launcher_lib::core::port;
use dsh_launcher_lib::core::process::{DshStatus, ProcessManager};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 本机是否具备真实 dsh 运行环境（clone 目录 + node）
fn dsh_env_available() -> bool {
    let Ok(local) = std::env::var("LOCALAPPDATA") else {
        return false;
    };
    let clone = PathBuf::from(&local)
        .join("dsh-launcher")
        .join("github-dsh")
        .join("deepseek-harness");
    clone.join("apps/cli/src/bin.ts").exists() && clone.join("package.json").exists()
}

/// 把本次用例的数据目录隔离到独占临时目录（日志/缓存都不碰用户环境）。
///
/// 必须在**任何** `Logger::init()` / `ProcessManager` 动作之前调用：`data_dir()`
/// 每次调用都读环境变量，故设置后立即生效。
fn isolate_data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dsh-launcher-e2e-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建临时数据目录失败");
    std::env::set_var("DSH_LAUNCHER_DATA_DIR", &dir);
    dir
}

/// 等待端口进入监听（有界）
fn wait_listening(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if port::is_port_in_use(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// 停止并等待端口释放（清理残留，避免污染后续用例）
fn cleanup(process: &ProcessManager, port: u16) {
    let _ = process.stop();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && port::is_port_in_use(port) {
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// RAII 清理：**断言失败/panic 时也必须**停掉本用例拉起的 dsh 并删除临时数据目录。
/// 若只写在测试末尾，panic 会跳过清理 → 残留进程占端口、污染后续运行。
struct TestGuard {
    process: Arc<ProcessManager>,
    port: u16,
    data_dir: PathBuf,
}

impl Drop for TestGuard {
    fn drop(&mut self) {
        cleanup(&self.process, self.port);
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

/// dsh 自带插件的**单实例互斥**判定：`ui-task-board` 的 ledger 由首个进程独占持有，
/// 第二个实例会在启动期失败（"task-board ledger is already owned by process <pid>"）。
///
/// 这是 dsh 自身的设计约束，不是本项目的缺陷；但它会让"并发拉起第二个 dsh"的用例
/// 必然失败。识别出来是为了给出**明确的前置条件提示**，而不是让用户看到一个
/// 含义不明的"token 未捕获"超时。
fn blocked_by_concurrent_instance(stderr: &str) -> Option<u32> {
    let marker = "is already owned by process ";
    let idx = stderr.find(marker)?;
    let rest = &stderr[idx + marker.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

#[test]
#[ignore = "需要真实 deepseek-harness 安装目录 + node（本地/nightly 集成验证）"]
fn 冷启动超过八秒上限时状态仍收敛为运行中() {
    assert!(
        dsh_env_available(),
        "本用例需要真实 deepseek-harness 安装目录（%LOCALAPPDATA%\\dsh-launcher\\github-dsh\\deepseek-harness）"
    );

    let port: u16 = 31987;
    assert!(
        !port::is_port_in_use(port),
        "端口 {port} 已被占用，请先释放或改用其他端口"
    );
    let data_dir = isolate_data_dir("converge");
    println!("隔离数据目录: {}", data_dir.display());

    let logger = Arc::new(Logger::init());
    let process = Arc::new(ProcessManager::new(Arc::clone(&logger)));

    let started = Instant::now();
    process
        .start(port)
        .expect("启动 dsh 失败（检查 node 与安装目录）");
    // RAII：此后任何断言失败/panic 都会停进程 + 删临时目录
    let _guard = TestGuard {
        process: Arc::clone(&process),
        port,
        data_dir: data_dir.clone(),
    };
    assert_eq!(
        process.status(),
        DshStatus::Starting,
        "spawn 后应立即处于 Starting"
    );

    // 每 200ms 调用一次按需对账（等价于后台 5s 线程 + 前端期间的轮询），
    // 直到状态收敛或超时。**不依赖 spawn_startup_probe 的 8 秒预算**。
    let deadline = Instant::now() + Duration::from_secs(150);
    let mut converged: Option<Instant> = None;
    while Instant::now() < deadline {
        process.reconcile_now();
        match process.status() {
            DshStatus::Running => {
                converged = Some(Instant::now());
                break;
            }
            DshStatus::Stopped => {
                let stderr = std::fs::read_to_string(
                    dsh_launcher_lib::core::process::dsh_stderr_path(),
                )
                .unwrap_or_default();
                // dsh 自带插件的单实例互斥：非本项目缺陷，给出明确前置条件
                if let Some(owner) = blocked_by_concurrent_instance(&stderr) {
                    panic!(
                        "本用例需要**独占**运行的 dsh，但已有 dsh 实例（pid={owner}）持有 \
                         ui-task-board 的 ledger 锁，第二个实例无法启动。\n\
                         这是 dsh 自身的设计约束。请先停止该 dsh（或改用其他 DSH_HOME）后重跑。"
                    );
                }
                panic!("dsh 启动即崩（状态回 Stopped）。stderr:\n{stderr}");
            }
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    let Some(converged) = converged else {
        panic!("150 秒内状态未收敛为 Running（正是本 BUG 的症状）");
    };

    println!(
        "✅ 状态收敛为 Running，用时 {:.2}s（旧实现在 8s 上限后永不收敛）",
        converged.duration_since(started).as_secs_f64()
    );

    // 托管身份：前端据此区分"继续等待"与"询问接管"
    assert!(
        process.is_managed(),
        "由启动器 start() 拉起的实例必须报告为受托管（pid != 0）"
    );
    assert_eq!(process.current_port(), port, "记录的端口应为启动端口");

    // 等到 token URL 被捕获（tail dsh stdout 落盘文件）——这是内嵌窗口免认证打开的前提
    let url_deadline = Instant::now() + Duration::from_secs(90);
    let mut url: Option<String> = None;
    while Instant::now() < url_deadline {
        let u = process.web_url();
        if u.contains("token=") {
            url = Some(u);
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let url = url.expect("90 秒内应从 dsh stdout 捕获带 token 的访问地址");
    println!("✅ 已捕获访问地址: {url}");

    // HTTP 层就绪（前端开窗前的最后一道门槛）
    assert!(
        port::web_ready(&url, 3000),
        "捕获到的地址应通过 HTTP 就绪探测（2xx/3xx）"
    );

    println!("✅ 清理由 TestGuard 完成（进程停止 + 临时目录删除）");
}

/// 收养路径：外部实例的**陈旧 token 不得复用**（v0.9.1 修复的第二处成因）。
///
/// 复现方式（无需真实外部 dsh 进程）：真实拉起一个 dsh 充当"命令行形如 dsh 的监听者"，
/// 然后向缓存写入一段**失效**地址（token 属于一个连不上的端口），再对该实例调用
/// `adopt_running`。若实现仍无条件沿用缓存，`web_url()` 会返回那段失效地址
/// —— 前端据此探测恒失败 → 空转 40 秒后报"访问地址无效"。
#[test]
#[ignore = "需要真实 deepseek-harness 安装目录 + node（本地/nightly 集成验证）"]
fn 收养外部实例不得复用陈旧_token() {
    assert!(
        dsh_env_available(),
        "本用例需要真实 deepseek-harness 安装目录"
    );

    let port: u16 = 31988;
    assert!(
        !port::is_port_in_use(port),
        "端口 {port} 已被占用，请先释放或改用其他端口"
    );
    let data_dir = isolate_data_dir("adopt");

    let logger = Arc::new(Logger::init());
    let starter = Arc::new(ProcessManager::new(Arc::clone(&logger)));
    starter.start(port).expect("启动 dsh 失败");
    // RAII：任何断言失败/panic 都停进程 + 删临时目录（不残留、不污染）
    let _guard = TestGuard {
        process: Arc::clone(&starter),
        port,
        data_dir: data_dir.clone(),
    };

    if !wait_listening(port, Duration::from_secs(150)) {
        let stderr =
            std::fs::read_to_string(dsh_launcher_lib::core::process::dsh_stderr_path())
                .unwrap_or_default();
        if let Some(owner) = blocked_by_concurrent_instance(&stderr) {
            panic!(
                "本用例需要**独占**运行的 dsh，但已有 dsh 实例（pid={owner}）持有 \
                 ui-task-board 的 ledger 锁，第二个实例无法启动。\n\
                 这是 dsh 自身的设计约束。请先停止该 dsh（或改用其他 DSH_HOME）后重跑。"
            );
        }
        panic!("dsh 未在 150 秒内监听端口 {port}。stderr:\n{stderr}");
    }

    // 端口已监听之后再注入失效缓存，确保输入确定（不受 starter tail 线程后续捕获
    // 真 token 影响）：该地址指向端口 1，必然连不上。
    dsh_launcher_lib::core::logging::save_latest_web_url(
        "http://127.0.0.1:1/?token=stale-token-from-dead-process",
    );

    // 另起一个 ProcessManager 模拟"启动器重开"：它对上述实例只能收养
    let adopter = ProcessManager::new(Arc::clone(&logger));
    assert!(
        adopter.adopt_running(port),
        "监听者命令行形如 dsh，应被成功收养"
    );
    assert_eq!(adopter.status(), DshStatus::Running);
    assert!(!adopter.is_managed(), "收养的实例不属于本启动器托管");

    // 核心断言：**不得**把那段陈旧 token 当作可用地址
    let url = adopter.web_url();
    assert!(
        !url.contains("stale-token-from-dead-process"),
        "收养时不得沿用陈旧 token（会导致前端空转 40 秒），实际: {url}"
    );
    assert!(
        url.is_empty(),
        "失效缓存应被清空（而非保留），实际: {url}"
    );
    // 缓存文件本身也必须清除，否则 get_web_url 的兜底仍会交出旧 token
    assert!(
        dsh_launcher_lib::core::logging::extract_latest_web_url().is_none(),
        "磁盘缓存必须同步清除（extract_latest_web_url 不得再返回旧 token）"
    );
    // 清理由 TestGuard 完成
}
