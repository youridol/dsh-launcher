//! 端口探测：判断 dsh web 是否在监听、端口是否被占用
//!
//! 语义（docs/DESIGN.md §4.3）：
//! - 启动前预检：配置端口被占时提示用户手动改（不自动递增，Q18 裁定）
//! - 运行状态检测：每 5 秒端口探活兜底（Q27 裁定）

use std::io::{self, Write};
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

/// HTTP 层就绪探测：对指定 URL（带 token 的完整 dsh web 地址）发 GET，
/// 收到 **200** 才视为"可服务"。用于开内嵌窗口前确认 dsh web 真正就绪——
/// 端口监听（TCP）≠ HTTP 可服务：冷启动时 dsh 先监端口、后挂路由，期间请求
/// 返回 **404**（SPA 路由未就绪），此时开窗 WebView2 首载即命中 404 错误页且
/// 不会自动恢复（须关闭重开）。返回 false 表示未就绪（连接失败/非 200/超时）。
///
/// 实现：TcpStream 手写最小 HTTP/1.1 GET（只读状态行，不引第三方依赖）。
/// url 形如 `http://127.0.0.1:3080/?token=xxx`。
pub fn web_ready(url: &str, timeout_ms: u64) -> bool {
    let Some((host, port, path)) = parse_http_url(url) else {
        return false;
    };
    // host 可能是 localhost/::1（非 IP 字面量），需解析为 SocketAddr
    use std::net::ToSocketAddrs;
    let Ok(mut addrs) = (host.as_str(), port).to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(1000)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(timeout_ms)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(timeout_ms)));
    // HTTP/1.1 GET：Host 头 + Connection: close（读完即断，不解析 body）
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nUser-Agent: dsh-launcher-probe\r\n\r\n"
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    use std::io::{BufRead, BufReader};
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    if reader.read_line(&mut status_line).is_err() {
        return false;
    }
    // 状态行形如 "HTTP/1.1 200 OK"
    status_line.starts_with("HTTP/1.1 200") || status_line.starts_with("HTTP/1.0 200")
}

/// 极简解析 `http://host:port/path?query` → (host, port, path?query)
/// 仅支持 http://127.0.0.1 / localhost / ::1；失败返回 None。
fn parse_http_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = if authority.starts_with('[') {
        // [::1]:3080
        let close = authority.find(']')?;
        let h = authority[1..close].to_string();
        let p = authority[close + 1..].strip_prefix(':')?.parse().ok()?;
        (h, p)
    } else if let Some(idx) = authority.rfind(':') {
        let h = authority[..idx].to_string();
        let p = authority[idx + 1..].parse().ok()?;
        (h, p)
    } else {
        (authority.to_string(), 80)
    };
    Some((host, port, path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::parse_http_url;

    #[test]
    fn test_parse_http_url() {
        assert_eq!(
            parse_http_url("http://127.0.0.1:3080/?token=abc").unwrap(),
            ("127.0.0.1".to_string(), 3080, "/?token=abc".to_string())
        );
        assert_eq!(
            parse_http_url("http://localhost:3080/").unwrap(),
            ("localhost".to_string(), 3080, "/".to_string())
        );
        assert_eq!(
            parse_http_url("http://[::1]:3080/x?y=1").unwrap(),
            ("::1".to_string(), 3080, "/x?y=1".to_string())
        );
        // 非法输入
        assert!(parse_http_url("").is_none());
        assert!(parse_http_url("https://127.0.0.1:3080/").is_none());
        assert!(parse_http_url("not a url").is_none());
    }
}
