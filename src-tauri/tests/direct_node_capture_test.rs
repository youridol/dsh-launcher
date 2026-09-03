//! 集成测试：验证 GitHub 通道 dsh 用"直接 node 启动"能实时捕获 token URL
//!
//! 背景（v0.4.2 修复）：旧实现走 dsh.cmd shim（cd 目录 && pnpm dsh），
//! Windows 下 pnpm 多层 cmd/node 嵌套导致 dsh web 的 token stdout 不实时到达
//! 启动器管道（实测延迟到进程被杀），内嵌窗口拿不到新 token → 401。
//! 本测试模拟 ProcessManager::start_locked 的 direct_node_cmd 链路：
//! node --import tsx/esm apps/cli/src/bin.ts web --port <port> --no-open，
//! cwd = github 安装目录，stdout 管道实时读，断言 token URL 在 25 秒内出现。
//!
//! 前置：本机已安装 deepseek-harness（github-dsh 目录）且 node 可执行。
//! 找不到环境时跳过（不失败）。

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn node_exe() -> Option<std::path::PathBuf> {
    // 优先用户级 node_dir，回退系统 nodejs（D:\Program Files\nodejs），再回退 PATH
    let user_node = std::env::var("LOCALAPPDATA")
        .map(|l| {
            std::path::PathBuf::from(l)
                .join("dsh-launcher")
                .join("toolchain")
                .join("node")
                .join("node.exe")
        })
        .ok()
        .filter(|p| p.exists());
    if let Some(p) = user_node {
        return Some(p);
    }
    // 常见系统 nodejs 安装目录（避免命中 Pi Agent 内部 node 等特殊环境）
    for cand in [
        "C:\\Program Files\\nodejs\\node.exe",
        "D:\\Program Files\\nodejs\\node.exe",
    ] {
        let p = std::path::PathBuf::from(cand);
        if p.exists() {
            return Some(p);
        }
    }
    let out = Command::new("where").arg("node").output().ok()?;
    if out.status.success() {
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(line) = text.lines().next() {
            let p = std::path::PathBuf::from(line.trim());
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

fn github_dir() -> Option<std::path::PathBuf> {
    let d = std::env::var("LOCALAPPDATA")
        .map(|l| {
            std::path::PathBuf::from(l)
                .join("dsh-launcher")
                .join("github-dsh")
                .join("deepseek-harness")
        })
        .ok()?;
    if d.join("package.json").exists() && d.join("apps/cli/src/bin.ts").exists() {
        Some(d)
    } else {
        None
    }
}

/// 杀掉进程树（清理用）
fn kill_tree(child: &mut Child) {
    let _ = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .output();
}

#[test]
#[ignore = "需要真实 deepseek-harness 安装目录 + node 环境（本地集成验证用）"]
fn test_direct_node_captures_token_url() {
    let Some(node) = node_exe() else {
        eprintln!("无 node.exe，跳过（非失败）");
        return;
    };
    let Some(gh) = github_dir() else {
        eprintln!("无 deepseek-harness 安装目录，跳过（非失败）");
        return;
    };

    let port = 31234u16;

    // 模拟 direct_node_cmd：node 绝对路径 + cwd=github + web --port --no-open
    let mut cmd = Command::new(&node);
    cmd.current_dir(&gh)
        .args([
            "--import",
            "tsx/esm",
            "apps/cli/src/bin.ts",
            "web",
            "--port",
            &port.to_string(),
            "--no-open",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().expect("spawn dsh (direct node) 失败");

    // stdout 读线程：逐行发 channel（命中 token= 即退出）
    let stdout = child.stdout.take().expect("stdout 管道");
    let (tx, rx) = mpsc::channel::<String>();
    let tx2 = tx.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut r = std::io::BufReader::new(stdout);
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = r.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            for &b in &chunk[..n] {
                if b == b'\n' {
                    if !buf.is_empty() {
                        let line = String::from_utf8_lossy(&buf).trim().to_string();
                        let _ = tx2.send(line);
                        buf.clear();
                    }
                } else {
                    buf.push(b);
                }
            }
        }
    });
    drop(tx); // 主线程保留 rx，释放自身 tx 使通道在 reader 退出后关闭

    // 主线程 25 秒内收 token 行（模拟 spawn_monitor 的 extract_web_url）
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut got_url: Option<String> = None;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                if let Some(idx) = line.find("http://127.0.0.1:") {
                    let rest = &line[idx..];
                    let end = rest
                        .find(|c: char| c.is_whitespace())
                        .unwrap_or(rest.len());
                    let url = &rest[..end];
                    if url.contains("token=") {
                        got_url = Some(url.to_string());
                        break;
                    }
                }
            }
            Err(_) => {
                // recv 超时：查进程是否已退出（启动即崩）
                if let Ok(Some(_)) = child.try_wait() {
                    eprintln!("dsh 进程提前退出，未捕获 token");
                    break;
                }
            }
        }
    }
    // 先杀进程树（关闭 stdout 管道 → reader 线程退出）再 join，避免 join 永久阻塞
    kill_tree(&mut child);
    let _ = reader_thread.join();

    let url = got_url.expect("25 秒内应从 dsh stdout 实时捕获带 token 的 URL");
    println!("✅ 直接 node 启动实时捕获 token URL: {url}");
    assert!(
        url.contains(&format!(":{port}/")),
        "URL 端口应为 {port}: {url}"
    );
}
