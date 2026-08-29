//! 端口探测：判断 dsh web 是否在监听、端口是否被占用
//!
//! 语义（docs/DESIGN.md §4.3）：
//! - 启动前预检：配置端口被占时提示用户手动改（不自动递增，Q18 裁定）
//! - 运行状态检测：每 5 秒端口探活兜底（Q27 裁定）

use std::io;
use std::net::TcpStream;
use std::time::Duration;

/// 尝试连接端口，判断是否有进程监听。
/// 返回三态：
/// - `Some(true)`  端口有进程监听（占用）
/// - `Some(false)` 明确无监听（ConnectionRefused）
/// - `None`        无法判定（超时/其他错误，如防火墙拦截）
pub fn probe(port: u16) -> Option<bool> {
    let addr = format!("127.0.0.1:{port}");
    let Ok(sock_addr) = addr.parse() else {
        return None;
    };
    match TcpStream::connect_timeout(&sock_addr, Duration::from_millis(500)) {
        Ok(_) => Some(true),
        Err(e) => match e.kind() {
            io::ErrorKind::ConnectionRefused => Some(false),
            io::ErrorKind::TimedOut => None,
            _ => None,
        },
    }
}

/// 端口是否被占用（启动前预检用）
pub fn is_port_in_use(port: u16) -> bool {
    probe(port) == Some(true)
}

/// 校验端口号合法性
pub fn validate_port(port: u16) -> bool {
    (1..=65535).contains(&port)
}
