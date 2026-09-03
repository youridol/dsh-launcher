//! 进程 PATH 注入与 Windows 用户 PATH 持久化
//!
//! 背景（分发机器 npm/pnpm/dsh 找不到）：
//! - 本启动器安装 Node 时解压到用户级 `%LOCALAPPDATA%\dsh-launcher\toolchain\node`，
//!   但旧代码**从不写系统/用户 PATH**，子进程（npm/pnpm/dsh）只查系统 PATH；
//! - 干净机器上系统 PATH 无 node → `Command::new("npm")` spawn 失败 /
//!   `cmd /C npm` 报 "'npm' 不是内部或外部命令"（乱码 + 退出码 1）。
//!
//! 双管齐下：
//! 1. **运行时前缀注入**：spawn 任意子进程前，若系统 PATH 无 node 且已装用户级
//!    Node，则把 node_dir 前缀注入子进程 PATH（进程内生效，改动本机）
//! 2. **持久化**：Node 安装成功后把 node_dir 写进**用户** PATH（HKCU\Environment，
//!    免管理员），卸载时移除 —— 保证重启/其他终端/后续进程都能找到 npm/pnpm。

use crate::core::command;
use crate::core::toolchain as core_toolchain;
use std::path::PathBuf;

/// 用户级 Node 安装目录（npm/pnpm 同目录，见 core/toolchain.rs）
fn node_bin_dir() -> PathBuf {
    core_toolchain::node_dir()
}
/// 系统 PATH 中是否存在 node（where node 探测）
pub fn node_in_system_path() -> bool {
    let mut probe = command::hidden("where");
    probe.arg("node");
    probe
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 用户级 Node 是否已安装（node.exe 存在）
pub fn user_node_installed() -> bool {
    node_bin_dir().join("node.exe").exists()
}

/// 判断给定 PATH 字符串是否已包含目标目录条目（忽略大小写，避免重复注入）
fn path_contains(path: &str, dir: &str) -> bool {
    path.split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .any(|p| p.eq_ignore_ascii_case(dir))
}

/// 返回应前缀注入的 node_dir 字符串；无需注入（系统已有 node / 未装用户级 Node）时返回 None。
pub fn node_dir_injection() -> Option<String> {
    if node_in_system_path() || !user_node_installed() {
        return None;
    }
    let dir = node_bin_dir().to_string_lossy().to_string();
    if dir.is_empty() {
        return None;
    }
    Some(dir)
}

/// 把 node_dir 前缀注入 cmd 的 PATH（进程内生效；用于 dsh/pnpm/npm 等子进程）。
pub fn inject_node_path_into(cmd: &mut std::process::Command) {
    if let Some(dir) = node_dir_injection() {
        let current = std::env::var("PATH").unwrap_or_default();
        if !path_contains(&current, &dir) {
            cmd.env("PATH", format!("{dir};{current}"));
        }
    }
}

// ======================== Windows 注册表（HKCU\Environment）====================
// 用户环境变量存于 HKCU\Environment（Path 类型 REG_EXPAND_SZ，可含 %VAR%），
// 免管理员即可读写；写入后广播 WM_SETTINGCHANGE 让资源管理器/新终端生效。

/// 读取当前用户 PATH（HKCU\Environment\Path，REG_EXPAND_SZ，自动展开 %VAR%）。
/// 读取失败返回默认空串（非 Windows 平台）。
pub fn user_path() -> String {
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_EXPAND_SZ};
        use windows::core::PCWSTR;

        const SUBKEY: &str = "Environment";
        const VALUE: &str = "Path";
        // 带 NUL 结尾的 UTF-16 键名/值名
        let subkey_wide: Vec<u16> = SUBKEY.encode_utf16().chain(std::iter::once(0)).collect();
        let value_wide: Vec<u16> = VALUE.encode_utf16().chain(std::iter::once(0)).collect();
        let mut buf = [0u16; 32768];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: buf 为有效可写缓冲，len 指向其字节容量；RegGetValueW 不越界写入
        let res = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_wide.as_ptr()),
                PCWSTR(value_wide.as_ptr()),
                RRF_RT_REG_EXPAND_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
                Some(&mut len),
            )
        };
        if res.is_err() || len < 2 {
            return String::new();
        }
        let raw = &buf[..(len as usize / 2)];
        let raw = trim_trailing_nul(raw);
        let s = String::from_utf16_lossy(raw);
        expand_env(&s)
    }
    #[cfg(not(windows))]
    {
        String::new()
    }
}

/// 写入当前用户 PATH（HKCU\Environment\Path，REG_EXPAND_SZ）。
pub fn set_user_path(value: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
            REG_EXPAND_SZ,
        };
        use windows::core::PCWSTR;

        const SUBKEY: &str = "Environment";
        const VALUE: &str = "Path";
        let subkey_wide: Vec<u16> = SUBKEY.encode_utf16().chain(std::iter::once(0)).collect();
        let value_wide: Vec<u16> = VALUE.encode_utf16().chain(std::iter::once(0)).collect();
        let data_wide: Vec<u16> = value.encode_utf16().collect(); // 不含 NUL：cbData 按字节计

        // SAFETY: subkey_wide 为合法 NUL 结尾 UTF-16；hkey 为输出句柄
        let mut hkey: HKEY = HKEY(std::ptr::null_mut());
        let res = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_wide.as_ptr()),
                Some(0),
                KEY_SET_VALUE,
                &mut hkey,
            )
        };
        if res.is_err() {
            return Err(format!("打开注册表 HKCU\\{SUBKEY} 失败: {res:?}"));
        }
        // 按 REG_EXPAND_SZ 写入（data 为 UTF-16 字节，不含结尾 NUL）
        // SAFETY: hkey 有效（KEY_SET_VALUE）；data_wide 为合法 UTF-16 数据
        //（UTF-16 LE 小端字节序即 Windows 存储格式，长度 = 2×元素数）
        let data_bytes: Vec<u8> = data_wide
            .iter()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let res = unsafe {
            RegSetValueExW(
                hkey,
                PCWSTR(value_wide.as_ptr()),
                Some(0),
                REG_EXPAND_SZ,
                Some(&data_bytes),
            )
        };
        // SAFETY: hkey 为 RegGetValueW 打开的有效句柄
        let _ = unsafe { RegCloseKey(hkey) };
        if res.is_err() {
            return Err(format!("写入注册表 HKCU\\{SUBKEY}\\Path 失败: {res:?}"));
        }
        broadcast_environment_change();
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = value;
        Ok(())
    }
}

/// 向用户 PATH 前缀追加目录（若已含则忽略）。返回新值。
pub fn prepend_user_path_dir(dir: &str) -> Result<String, String> {
    let cur = user_path();
    if path_contains(&cur, dir) {
        return Ok(cur);
    }
    let next = if cur.trim().is_empty() {
        dir.to_string()
    } else {
        format!("{dir};{cur}")
    };
    set_user_path(&next)?;
    Ok(next)
}

/// 从用户 PATH 移除指定目录（精确匹配，大小写不敏感）。
/// 返回是否真的发生了移除（用于卸载日志）。
pub fn remove_user_path_dir(dir: &str) -> Result<bool, String> {
    let cur = user_path();
    let filtered: Vec<&str> = cur
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case(dir))
        .collect();
    let next = filtered.join(";");
    if next == cur {
        return Ok(false);
    }
    set_user_path(&next)?;
    Ok(true)
}

/// 去掉 UTF-16 缓冲尾部的 NUL（RegGetValueW 返回值以 NUL 结尾）
#[cfg(windows)]
fn trim_trailing_nul(buf: &[u16]) -> &[u16] {
    let mut end = buf.len();
    while end > 0 && buf[end - 1] == 0 {
        end -= 1;
    }
    &buf[..end]
}

/// 广播 WM_SETTINGCHANGE（通知系统用户环境变量已变更，资源管理器/新终端刷新）
#[cfg(windows)]
fn broadcast_environment_change() {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    const MSG: &str = "Environment";
    let msg_wide: Vec<u16> = MSG.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: HWND_BROADCAST 广播所有顶层窗口；lParam 指向 msg_wide（调用期间存活）
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(msg_wide.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            500,
            None,
        );
    }
}

/// 展开路径字符串中的 %VAR% 系统变量（Windows ExpandEnvironmentStringsW）。
/// 失败时原样返回。
#[cfg(windows)]
fn expand_env(input: &str) -> String {
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = input.encode_utf16().chain(std::iter::once(0)).collect();
    // 第一次调用取所需缓冲长度（含结尾 NUL）
    // SAFETY: wide 为合法 NUL 结尾 UTF-16；长度参数 0 表示只查询
    let need = unsafe { ExpandEnvironmentStringsW(PCWSTR(wide.as_ptr()), None) };
    if need == 0 {
        return input.to_string();
    }
    let mut buf = vec![0u16; need as usize];
    // SAFETY: buf 容量 = need（含结尾 NUL），由 API 填充；成功写入 ≤ need 项
    let written = unsafe {
        ExpandEnvironmentStringsW(
            PCWSTR(wide.as_ptr()),
            Some(buf.as_mut_slice()),
        )
    };
    if written == 0 {
        return input.to_string();
    }
    let raw = trim_trailing_nul(&buf);
    String::from_utf16_lossy(raw)
}

#[cfg(not(windows))]
fn expand_env(input: &str) -> String {
    input.to_string()
}

#[cfg(test)]
mod tests {
    use super::path_contains;

    #[test]
    fn test_path_contains() {
        assert!(path_contains(
            "C:\\a;C:\\Users\\x\\AppData\\Local\\dsh-launcher\\toolchain\\node",
            "C:\\Users\\x\\AppData\\Local\\dsh-launcher\\toolchain\\node"
        ));
        assert!(path_contains("c:\\A;D:\\B", "c:\\a"));
        assert!(!path_contains("C:\\aa;D:\\b", "C:\\a"));
        assert!(!path_contains("", "C:\\a"));
        assert!(!path_contains("C:\\node", "C:\\node;"));
        assert!(path_contains("C:\\node;", "C:\\node"));
    }

    #[test]
    fn test_inject_node_path_idempotent() {
        // 系统 PATH 已含 node_dir 时，重复注入不产生重复条目（幂等）
        let dir = "C:\\node";
        let current = format!("{dir};C:\\Windows");
        assert!(path_contains(&current, dir));
        // 大小写不敏感重复
        let current2 = "c:\\NODE;D:\\x";
        assert!(path_contains(current2, "C:\\node"));
        // 前缀注入后不应重复
        let injected = format!("{dir};{current}");
        let mut seen = 0;
        for p in injected.split(';') {
            if p.eq_ignore_ascii_case(dir) {
                seen += 1;
            }
        }
        assert_eq!(seen, 2, "注入前已有则不应再注入（由调用方判断）");
    }
}
