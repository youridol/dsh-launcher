//! 启动器配置持久化
//!
//! 配置存于 `%APPDATA%\dsh-launcher\config.json`。
//! 字段语义见 CONTEXT.md 与 docs/DESIGN.md。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// 启动器全局配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// dsh web 启动端口（用户自定义，默认 3080）
    pub port: u16,
    /// npm registry 镜像源（空 = 官方源）
    pub npm_registry: String,
    /// GitHub 加速镜像（clone / release 下载，空 = 官方源）
    pub github_mirror: String,
    /// GitHub Personal Access Token（防 API 限流；git 认证增强，空 = 匿名）
    pub github_token: String,
    /// Node 二进制下载镜像（空 = 官方 nodejs.org）
    pub node_mirror: String,
    /// 主窗口关闭按钮行为：true = 直接退出（含 dsh），false = 最小化到托盘
    pub close_exits: bool,
    /// 最小化到托盘
    pub minimize_to_tray: bool,
    /// 退出时驻留 dsh（不停止）
    pub keep_dsh_on_exit: bool,
    /// 卸载时保留 DSH_HOME
    pub keep_dsh_home_on_uninstall: bool,
    /// 启动时自动启动 dsh
    pub auto_start_dsh: bool,
    /// 启动时自动打开浏览器
    pub auto_open_browser: bool,
    /// 启动后自动同步 upstream 插件（自研插件永不受影响，见 ADR-0005 D6）
    pub auto_sync_plugins: bool,
    /// 外部编辑器命令（ADR-0008）：空 = 用系统默认关联程序打开。
    ///
    /// 自由文本而非枚举 —— 编辑器无法穷举（VS Code / Cursor / Zed / Sublime /
    /// Notepad++ …）。只取第一段作为可执行文件，其余作为参数（支持
    /// `code --wait` 这类带参数的命令行）。
    pub editor_command: String,
    /// 是否已经问过「用哪个程序打开」：首次点击编辑时弹一次引导，选完置 true。
    pub editor_prompt_seen: bool,
}

impl Default for AppConfig {
    /// 默认配置：端口 3080（DEFAULT_PORT），其余字段取类型默认
    fn default() -> Self {
        Self {
            port: Self::DEFAULT_PORT,
            npm_registry: String::new(),
            github_mirror: String::new(),
            github_token: String::new(),
            node_mirror: String::new(),
            close_exits: false,
            minimize_to_tray: true,
            keep_dsh_on_exit: false,
            keep_dsh_home_on_uninstall: true,
            auto_start_dsh: false,
            auto_open_browser: true,
            auto_sync_plugins: true,
            editor_command: String::new(),
            editor_prompt_seen: false,
        }
    }
}

impl AppConfig {
    /// 默认端口
    pub const DEFAULT_PORT: u16 = 3080;

    /// 计算配置文件的默认路径
    pub fn config_path() -> PathBuf {
        dirs_config_dir().join("dsh-launcher").join("config.json")
    }

    /// 计算配置文件路径的默认路径与读写入口见上。
    ///
    /// 返回配置的 npm registry（去首尾空白）；为空表示使用官方/用户默认源。
    /// 供 npm/pnpm 网络操作注入 `--registry`（审计修复 2.1：此前镜像配置从不生效）。
    pub fn npm_registry_url(&self) -> Option<String> {
        let s = self.npm_registry.trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    }
}

/// 读取当前配置的 npm registry（为空时返回 None）。
/// 独立免费函数便于在各命令构造点直接调用（审计修复 2.1）。
pub fn current_npm_registry() -> Option<String> {
    AppConfig::load().npm_registry_url()
}

/// 配置加载中可恢复的问题（供调用方落日志；G5 / 审计 RT-03）。
///
/// 此前 `AppConfig::load` 对「JSON 解析失败」与「DPAPI 解密失败」均静默回退/置空，
/// 用户配置（端口/开关/镜像）一次性丢失后**无任何可观测线索**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadIssue {
    /// 配置文件存在但解析失败（已回退默认配置）
    ParseFailed(String),
    /// `github_token` 为 `dpapi1:` 密文但解密失败（已置空，见审计 2.4）
    TokenDecryptFailed,
}

impl LoadIssue {
    /// 面向用户/日志的中文说明。
    pub fn message(&self) -> String {
        match self {
            LoadIssue::ParseFailed(detail) => format!(
                "配置文件解析失败，已回退默认配置（原文件未被覆盖）：{detail}"
            ),
            LoadIssue::TokenDecryptFailed => {
                "GitHub Token 解密失败（可能因更换用户或机器），已置空；请重新设置".to_string()
            }
        }
    }
}

impl AppConfig {
    /// 读取配置；文件不存在时返回默认配置
    ///
    /// v0.4.13（审计修复 2.4）：github_token 字段若带 `dpapi1:` 前缀则按 DPAPI
    /// 解密（Windows）；解不开（换用户/机器）时置空，避免以密文当明文使用。
    pub fn load() -> Self {
        Self::load_checked().0
    }

    /// 带问题的配置读取（G5 / 审计 RT-03）：返回配置与**可选的加载问题**。
    ///
    /// 文件不存在视为正常首启（无问题）；仅在「存在但解析失败」或
    /// 「token 解密失败」时返回 `Some(issue)`，供调用方（如 `get_config`）落日志。
    pub fn load_checked() -> (Self, Option<LoadIssue>) {
        let path = Self::config_path();
        let Ok(content) = fs::read_to_string(&path) else {
            // 不存在 / 不可读：保持旧行为（默认配置），不当作异常
            return (Self::default(), None);
        };
        let mut v = match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(v) => v,
            Err(e) => {
                return (
                    Self::default(),
                    Some(LoadIssue::ParseFailed(e.to_string())),
                )
            }
        };
        let mut issue = None;
        if let Some(s) = v.get("github_token").and_then(|t| t.as_str()) {
            if s.starts_with(DPAPI_PREFIX) {
                match decrypt_token(s) {
                    Some(dec) => v["github_token"] = serde_json::Value::String(dec),
                    None => {
                        v["github_token"] = serde_json::Value::String(String::new());
                        issue = Some(LoadIssue::TokenDecryptFailed);
                    }
                }
            }
        }
        match serde_json::from_value(v) {
            Ok(config) => (config, issue),
            Err(e) => (
                Self::default(),
                Some(LoadIssue::ParseFailed(e.to_string())),
            ),
        }
    }

    /// 写入配置
    ///
    /// v0.4.13（审计修复 2.6/2.4）：
    /// - 改为“临时文件 + 替换”的原子写（避免崩溃截断半截 JSON）；
    /// - github_token 落盘前用 DPAPI 加密（Windows），密文带 `dpapi1:` 前缀。
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut v = serde_json::to_value(self).map_err(|e| e.to_string())?;
        if let Some(token) = v.get("github_token").and_then(|t| t.as_str()) {
            if !token.is_empty() {
                // v0.4.15（审计修复 3.x）：加密失败不再静默回退明文落盘——
                // 此前 `unwrap_or_else(|| token.to_string())` 在 CryptProtectData 失败
                // （策略限制/异常）时把 token 明文写盘且无任何提示。改为返回 Err，
                // 由调用方（set_github_token）提示用户保存失败，绝不降级明文持久化。
                let enc = encrypt_token(token).ok_or_else(|| {
                    "GitHub Token 加密失败（DPAPI 不可用），已取消保存以防明文落盘".to_string()
                })?;
                v["github_token"] = serde_json::Value::String(enc);
            }
        }
        let json = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).map_err(|e| e.to_string())?;
        // Windows 下 rename 无法覆盖已存在文件：先删除旧文件再 rename（间隙极小）
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
        fs::rename(&tmp, &path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            e.to_string()
        })
    }
}

/// 定位系统配置目录（Windows: %APPDATA%）
fn dirs_config_dir() -> PathBuf {
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

// ==================== GitHub Token 落盘加密（审计修复 2.4）====================
// Windows 用 DPAPI（CryptProtectData，当前用户域加密）保护 github_token，
// 密文以 "dpapi1:" + hex 形式落盘；解密失败（换用户/机器）返回 None → 置空。
// 非 Windows（开发环境）保持明文，与旧行为一致。

const DPAPI_PREFIX: &str = "dpapi1:";

#[cfg(windows)]
fn encrypt_token(plain: &str) -> Option<String> {
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};
    use windows::Win32::Foundation::{LocalFree, HLOCAL};

    let bytes = plain.as_bytes();
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: in_blob 指向合法明文；out_blob 由 API 分配，成功路径用 LocalFree 释放；
    // CRYPTPROTECT_UI_FORBIDDEN(0x01) 禁止弹出 UI。
    let ok = unsafe {
        CryptProtectData(
            &in_blob,
            None,
            None,
            None,
            None,
            0x01,
            &mut out_blob,
        )
    };
    if ok.is_err() {
        return None;
    }
    // SAFETY: out_blob 由 CryptProtectData 填充，长度 cbData 有效
    let data = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) }
        .to_vec();
    // SAFETY: 释放 API 分配的缓冲
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out_blob.pbData as _)));
    }
    Some(hex_encode(&data))
}

#[cfg(windows)]
fn decrypt_token(enc: &str) -> Option<String> {
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    use windows::Win32::Foundation::{LocalFree, HLOCAL};

    let hex_part = enc.strip_prefix(DPAPI_PREFIX)?;
    let data = hex_decode(hex_part)?;
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: 同上；解密以当前用户上下文进行
    let ok = unsafe { CryptUnprotectData(&in_blob, None, None, None, None, 0, &mut out_blob) };
    if ok.is_err() {
        return None;
    }
    // SAFETY: out_blob 由 API 填充
    let data = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) }
        .to_vec();
    // SAFETY: 释放 API 分配的缓冲
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out_blob.pbData as _)));
    }
    String::from_utf8(data).ok()
}

#[cfg(not(windows))]
fn encrypt_token(plain: &str) -> Option<String> {
    Some(plain.to_string())
}

#[cfg(not(windows))]
fn decrypt_token(enc: &str) -> Option<String> {
    enc.strip_prefix(DPAPI_PREFIX).map(|_| String::new())
}

/// 小端十六进制编码（不引入外部依赖）
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// 小端十六进制解码（非法输入返回 None）
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let b = s.as_bytes();
    for i in (0..b.len()).step_by(2) {
        let hi = (b[i] as char).to_digit(16)?;
        let lo = (b[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}
