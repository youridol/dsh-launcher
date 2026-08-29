//! 统一进程创建辅助：禁止弹出黑色控制台窗口
//!
//! Windows 下所有 spawn 的子进程（dsh、npm、git、pnpm、curl、taskkill、powershell）
//! 默认会继承控制台或弹出新窗口。这里统一应用：
//! - `CREATE_NO_WINDOW` (0x08000000)：不创建新控制台窗口
//! - `DETACHED_PROCESS` (0x00000008)：不继承父进程控制台
//! 两者组合 = 子进程完全无可见窗口。
//!
//! 用法：`let mut cmd = command::hidden("npm"); cmd.args([...]);`
//! 非 Windows 平台直接返回普通 Command（编译期 cfg 隔离）。

/// CREATE_NO_WINDOW：不创建控制台窗口
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// DETACHED_PROCESS：脱离父进程控制台
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;

/// 创建一个已配置为"无窗口"的 Command
/// program 为要执行的可执行文件（如 "npm"、"git"、完整路径）
#[cfg(windows)]
pub fn hidden<S: AsRef<std::ffi::OsStr>>(program: S) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    cmd
}

/// 非 Windows 平台：普通 Command
#[cfg(not(windows))]
pub fn hidden<S: AsRef<std::ffi::OsStr>>(program: S) -> std::process::Command {
    std::process::Command::new(program)
}
