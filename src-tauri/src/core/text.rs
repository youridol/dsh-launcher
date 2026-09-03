//! 子进程输出文本解码：UTF-8 优先、失败回退 Windows 代码页（GBK/OEM）
//!
//! 背景（乱码根因）：
//! - 中文 Windows 默认 ACP=936（GBK），`cmd.exe` 内建错误消息、taskkill、
//!   PowerShell 5.1 等向管道输出 **GBK/当前 OEM 代码页** 字节；
//! - 旧代码到处用 `String::from_utf8_lossy` 按 UTF-8 硬解 → 中文全变 `������`；
//! - 真正的 UTF-8 程序（node/npm/pnpm/git 等 Unicode 输出）必须优先按 UTF-8 解，
//!   不能直接按 GBK 解（否则中文 UTF-8 字节会被 GBK 误拆成乱码）。
//!
//! 因此统一策略 = **UTF-8 严格解码优先，失败回退代码页解码**（本模块唯一实现）。

use std::borrow::Cow;

/// 按 UTF-8 严格解码；失败（含非法序列）回退 Windows 系统代码页解码。
/// 输入为整段缓冲（可能含换行）。
pub fn decode(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    // UTF-8 非法 → 按系统代码页（中文 Windows 为 GBK，欧美为 CP1252 等）解码
    decode_codepage(bytes).into_owned()
}

/// 按当前 ANSI 代码页解码（UTF-8 解码失败时的兜底）。
/// 用系统 API（GetACP）而非硬编码 GBK：非中文系统（CP437/1252）也能正确解码。
fn decode_codepage(bytes: &[u8]) -> Cow<'_, str> {
    let cp = ansi_code_page();
    match cp {
        // 中文 Windows：ACP=936（GBK）
        936 => {
            let (cow, _, _) = encoding_rs::GBK.decode(bytes);
            cow
        }
        // 欧美 Windows：ACP=1252（Windows-1252）
        1252 => {
            let (cow, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
            cow
        }
        // 老式 OEM 代码页（无 encoding_rs 直接支持，IBM866 为近似）
        437 | 866 => {
            let (cow, _, _) = encoding_rs::IBM866.decode(bytes);
            cow
        }
        // 其他：按 Windows-1252 近似解码（保 ASCII 内容）
        _ => {
            let (cow, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
            cow
        }
    }
}

/// 当前 ANSI 代码页（GetACP；失败返回 1252 兜底）
fn ansi_code_page() -> u32 {
    #[cfg(windows)]
    {
        // SAFETY: GetACP 无参数、无失败路径，线程安全
        unsafe { windows::Win32::Globalization::GetACP() }
    }
    #[cfg(not(windows))]
    {
        1252
    }
}

/// 当前系统是否中文（ACP=936/GBK）——仅测试辅助（跨模块 GBK 测试用）。
/// 生产代码不依赖系统语言分支，统一按 GetACP 动态解码。
#[cfg(test)]
pub fn is_cjk_system_for_test() -> bool {
    ansi_code_page() == 936
}

#[cfg(test)]
mod tests {
    use super::ansi_code_page;
    use super::decode;

    /// 当前系统是否为中文（ACP=936/GBK）环境。
    /// GBK 解码单测仅在中文环境断言中文还原；非中文环境（CI 英文 runner ACP=1252）
    /// 下 GBK 字节按本机代码页解码是符合设计的（见 decode 策略），只验证不崩溃。
    fn is_cjk_system() -> bool {
        ansi_code_page() == 936
    }

    #[test]
    fn test_utf8_passthrough() {
        // UTF-8 输入原样通过（node/npm/git 等 Unicode 输出）
        assert_eq!(decode("hello".as_bytes()), "hello");
        assert_eq!(decode("开始安装 pnpm…".as_bytes()), "开始安装 pnpm…");
    }

    #[test]
    fn test_gbk_cmd_error() {
        // 真实 cmd 错误（GBK）："'npm' 不是内部或外部命令，也不是可运行的程序或批处理文件。"
        let msg = "'npm' 不是内部或外部命令，也不是可运行的程序或批处理文件。\r\n请检查输入的拼写是否正确。";
        let gbk = encoding_rs::GBK.encode(msg).0;
        let decoded = decode(&gbk);
        if is_cjk_system() {
            assert!(decoded.contains("不是内部或外部命令"), "中文系统 GBK 解码应得中文: {decoded:?}");
            assert!(decoded.contains("请检查输入的拼写"), "应包含提示: {decoded:?}");
        } else {
            // 非中文系统（英文 CI runner）：GBK 字节按本机代码页解码（符合设计），
            // 仅验证解码不 panic 且保留 ASCII 前缀
            assert!(decoded.contains("npm"), "非中文系统也应保留 ASCII 片段: {decoded:?}");
        }
    }

    #[test]
    fn test_gbk_with_ascii_prefix() {
        // cmd 错误常带 ASCII 前缀（混合字节）
        let msg = "npm : 无法将“npm”项识别为 cmdlet、函数、脚本文件或可运行程序的名称。";
        let gbk = encoding_rs::GBK.encode(msg).0;
        let decoded = decode(&gbk);
        if is_cjk_system() {
            assert!(decoded.contains("无法将"), "中文系统混合字节应正确解码: {decoded:?}");
        } else {
            // 非中文系统：仅验证解码不 panic 且保留 ASCII 前缀
            assert!(decoded.contains("npm"), "非中文系统也应保留 ASCII 片段: {decoded:?}");
        }
    }

    #[test]
    fn test_unknown_bytes_no_panic() {
        // 非法字节组合不 panic
        let _ = decode(&[0xff, 0xfe, 0x00, 0x41]);
        let _ = decode(&[]);
    }
}
