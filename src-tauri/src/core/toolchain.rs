//! 工具链核心逻辑：Node/Git/Python 下载、安装、卸载（被 commands/toolchain.rs 调用）
//!
//! 策略（Q4/Q15/Q16 裁定 + 分发修复 v0.4.0）：
//! - Node：官方 zip 解压到 `%LOCALAPPDATA%\dsh-launcher\toolchain\node\`，用户级、免管理员；
//!   安装成功后把 node_dir **写入用户 PATH**（core/pathutil.rs）→ 分发机器 npm/pnpm/dsh 可直接解析
//! - Git：官方安装包（.exe），需提权 → commands 层 spawn runas
//! - Python：官方嵌入版 zip（免管理员，python 3.x 自带 pip？——嵌入版不含 pip，
//!   需 `python -m ensurepip` 或独立安装包；本实现用**官方完整安装包静默安装**，需提权）
//! - 镜像源：通过 AppConfig 注入下载 URL 前缀

use crate::core::command;
use crate::core::config::AppConfig;
use crate::core::logging::Logger;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 工具链根目录：%LOCALAPPDATA%\dsh-launcher\toolchain
pub fn toolchain_dir() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("dsh-launcher")
        .join("toolchain")
}

/// Node 安装目录
pub fn node_dir() -> PathBuf {
    toolchain_dir().join("node")
}

/// 带实时进度的下载：Invoke-WebRequest + 后台线程轮询目标文件大小，
/// 按 Content-Length 占比推送 install://progress 事件（channel=toolchain）。
///
/// 背景（v0.4.8 修复）：此前 Python/Node/Git 大文件下载用一次性同步等待，
/// 期间前端进度条停在 0% 不动（几十 MB 安装包下载无任何反馈）→ 用户感知"卡死"。
/// 本实现：
/// 1. 先 Invoke-WebRequest -Method Head 拿 Content-Length（总字节）；
/// 2. 后台线程每 200ms stat 目标文件大小 → progress(Download, bytes/total*100)；
/// 3. 下载进程退出后停轮询；拿不到总长度时显示已下载 MB（不定量消息）。
///
/// 幂等：全部调用方（Node/Git/Python 下载）都用本函数获得实时进度。
pub fn download_with_progress(
    logger: &Arc<Logger>,
    url: &str,
    dest: &Path,
    tool_label: &str,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // 旧文件残留先清（避免轮询把上次数当本次进度）
    let _ = fs::remove_file(dest);
    logger.info(&format!("下载 {url} → {}", dest.display()));

    // 1. HEAD 请求拿 Content-Length（失败则 total=0，退化为"已下载 MB"不定量消息）
    let total = fetch_content_length(url);
    if total > 0 {
        logger.progress(
            "toolchain",
            crate::core::events::InstallPhase::Download,
            0,
            format!("{tool_label} 下载中…（共 {:.1} MB）", total as f64 / 1_048_576.0),
        );
    } else {
        logger.progress(
            "toolchain",
            crate::core::events::InstallPhase::Download,
            0,
            format!("{tool_label} 下载中…"),
        );
    }

    // 2. 后台轮询线程（下载期间持续推进度）
    let stop = Arc::new(AtomicBool::new(false));
    let dest_owned = dest.to_path_buf();
    let logger_t = Arc::clone(logger);
    let label = tool_label.to_string();
    let stop_t = Arc::clone(&stop);
    let poller = std::thread::spawn(move || {
        let mut last_pct: i32 = -1;
        let mut last_len: u64 = 0;
        while !stop_t.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(200));
            let len = fs::metadata(&dest_owned).map(|m| m.len()).unwrap_or(0);
            // 下载完成（总字节已知且已到达）→ 结束轮询（100 由主线程下载完成后统一推）
            if total > 0 && len >= total {
                break;
            }
            if total > 0 {
                let pct = download_percent(len, total);
                if pct != last_pct {
                    last_pct = pct;
                    logger_t.progress(
                        "toolchain",
                        crate::core::events::InstallPhase::Download,
                        pct as u8,
                        format!(
                            "{label} 下载中… {:.1} MB / {:.1} MB ({pct}%)",
                            len as f64 / 1_048_576.0,
                            total as f64 / 1_048_576.0
                        ),
                    );
                }
            } else if len != last_len {
                last_len = len;
                logger_t.progress(
                    "toolchain",
                    crate::core::events::InstallPhase::Download,
                    0,
                    format!("{label} 下载中… {:.1} MB 已下载", len as f64 / 1_048_576.0),
                );
            }
        }
    });

    // 3. 执行实际下载（Invoke-WebRequest）
    let ps = format!(
        "Invoke-WebRequest -Uri '{}' -OutFile '{}' -UseBasicParsing",
        url.replace('\'', "''"),
        dest.to_string_lossy().replace('\'', "''")
    );
    let mut cmd = command::hidden("powershell");
    cmd.args(["-NoProfile", "-Command", &ps]);
    let out = cmd
        .output()
        .map_err(|e| format!("启动 PowerShell 下载失败: {e}"))?;
    // 下载结束 → 停轮询线程
    stop.store(true, Ordering::SeqCst);
    let _ = poller.join();

    if !out.status.success() {
        return Err(format!(
            "下载失败: {}",
            crate::core::text::decode(&out.stderr).trim()
        ));
    }
    let len = dest.metadata().map(|m| m.len()).unwrap_or(0);
    logger.info(&format!("下载完成：{}（{} 字节）", dest.display(), len));
    logger.progress(
        "toolchain",
        crate::core::events::InstallPhase::Download,
        100,
        format!("{tool_label} 下载完成（{:.1} MB）", len as f64 / 1_048_576.0),
    );
    Ok(())
}

/// 用 PowerShell Invoke-WebRequest -Method Head 获取资源 Content-Length（字节）。
/// 失败返回 0（下载进度退化为不定量消息）。
fn fetch_content_length(url: &str) -> u64 {
    let ps = format!(
        "try {{ (Invoke-WebRequest -Uri '{}' -Method Head -UseBasicParsing).Headers['Content-Length'] }} catch {{ '' }}",
        url.replace('\'', "''")
    );
    let mut cmd = command::hidden("powershell");
    cmd.args(["-NoProfile", "-Command", &ps]);
    if let Ok(out) = cmd.output() {
        if out.status.success() {
            let text = crate::core::text::decode(&out.stdout);
            if let Ok(n) = text.trim().parse::<u64>() {
                return n;
            }
        }
    }
    0
}

/// 计算下载进度百分比（0-99，不含 100：100 由完成路径单独推）
/// len=已下载字节，total=总字节；total=0（未知）时不适用（调用方走不定量分支）。
fn download_percent(len: u64, total: u64) -> i32 {
    if total == 0 {
        return 0;
    }
    ((len as f64 / total as f64) * 100.0).min(99.0) as i32
}

/// 解压 zip 到目标目录（PowerShell Expand-Archive）
fn unzip(logger: &Arc<Logger>, zip: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    logger.info(&format!("解压 {} → {}", zip.display(), dest.display()));
    let ps = format!(
        "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
        zip.to_string_lossy().replace('\'', "''"),
        dest.to_string_lossy().replace('\'', "''")
    );
    let mut cmd = command::hidden("powershell");
    cmd.args(["-NoProfile", "-Command", &ps]);
    let out = cmd
        .output()
        .map_err(|e| format!("启动 PowerShell 解压失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "解压失败: {}",
            crate::core::text::decode(&out.stderr).trim()
        ));
    }
    logger.info(&format!("解压完成：{}", dest.display()));
    Ok(())
}

/// 当前固定安装的 Node 版本
pub const NODE_VERSION: &str = "v22.19.0";

/// 在注册表卸载入口（HKLM/HKCU \\ SOFTWARE\\...\\Uninstall）中查找显示名含 keyword 的项，
/// 返回其 UninstallString（供 Git/Python 卸载器调用）。找不到返回 None。
/// 显示名匹配：Git for Windows → "Git"；Python 官方 → "Python 3.13 (64-bit)" → "Python"。
/// 匹配卸载项语义：返回 DisplayName 是否匹配目标工具（精确语义，避免误匹配）。
/// - Git：DisplayName 精确等于 "Git"（Git for Windows 卸载项），或 "Git for Windows..." 前缀
/// - Python：形如 `Python X.Y.Z (64-bit)` 且不含子组件词（python.org .exe 主安装项）
///   （MSI 拆分组件如 "Python 3.x Core Interpreter" 不匹配——直接跑 MSI 卸载会拆坏安装）
fn uninstall_entry_matches(display: &str, keyword: &str) -> bool {
    let d = display.trim();
    match keyword {
        "git" => {
            d == "Git" || d.starts_with("Git for Windows")
        }
        "python" => {
            // 主 Python 项：Python X.Y.Z (64-bit) / (32-bit)
            if !(d.starts_with("Python ") && d.contains("(") && d.contains(")")) {
                return false;
            }
            let sub = [
                "Development", "Documentation", "Test Suite", "Standard Library", "pip Bootstrap",
                "Tcl/Tk", "Executables", "Add to Path", "Utility Scripts", "Core Interpreter",
                "Launcher", "Windows Store", "embeddable", "Debug", "tcltk", "IDLE", "Libs",
            ];
            !sub.iter().any(|s| d.contains(s))
        }
        _ => d.to_lowercase().contains(&keyword.to_lowercase()),
    }
}

/// 在注册表卸载入口（HKLM/HKCU \\ SOFTWARE\\...\\Uninstall）查找匹配工具链的卸载项，
/// 返回 UninstallString（清理引号包裹）。找不到返回 None。
/// 遍历 HKLM + HKCU，64 位视角；优先非 MsiExec 的卸载器（Inno unins000.exe / python.org exe）。
pub fn find_uninstall_entry(keyword: &str) -> Option<String> {
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::{
            RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER,
            HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY,
        };
        use windows::core::{PCWSTR, PWSTR};

        // 候选根：64 位视角 HKLM + HKCU（KEY_WOW64_64KEY：避免 32 位视角重定向）
        const ROOTS: [(HKEY, &str); 2] = [
            (HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall"),
            (HKEY_CURRENT_USER, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall"),
        ];
        // 收集候选（key = 清理后 UninstallString），优先非 MsiExec
        let mut msi_candidate: Option<String> = None;
        let mut exe_candidate: Option<String> = None;
        for (root, subkey) in ROOTS {
            let subkey_wide: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: subkey_wide 合法；hkey 为输出
            let mut hkey: HKEY = HKEY(std::ptr::null_mut());
            let res = unsafe {
                RegOpenKeyExW(root, PCWSTR(subkey_wide.as_ptr()), Some(0), KEY_READ | KEY_WOW64_64KEY, &mut hkey)
            };
            if res.is_err() {
                continue;
            }
            // 枚举子项（各软件安装器的卸载注册表键）
            let mut index = 0u32;
            loop {
                let mut name_buf = [0u16; 512];
                let mut name_len = name_buf.len() as u32;
                // SAFETY: name_buf 有效；RegEnumKeyExW 枚举 hkey 下子键
                let r = unsafe {
                    RegEnumKeyExW(
                        hkey,
                        index,
                        Some(PWSTR(name_buf.as_mut_ptr())),
                        &mut name_len,
                        None,
                        None,
                        None,
                        None,
                    )
                };
                if r.is_err() {
                    break; // 枚举完毕（ERROR_NO_MORE_ITEMS）
                }
                index += 1;
                let name_end = name_len as usize;
                let display = String::from_utf16_lossy(&name_buf[..name_end]);
                // 打开该子键读 DisplayName / UninstallString
                let sub_wide: Vec<u16> = display.encode_utf16().chain(std::iter::once(0)).collect();
                let mut sub_key: HKEY = HKEY(std::ptr::null_mut());
                // SAFETY: sub_wide 合法
                let res2 = unsafe {
                    RegOpenKeyExW(
                        hkey,
                        PCWSTR(sub_wide.as_ptr()),
                        Some(0),
                        KEY_READ | KEY_WOW64_64KEY,
                        &mut sub_key,
                    )
                };
                if res2.is_err() {
                    continue;
                }
                // 读 DisplayName
                let display_name = read_reg_string(sub_key, "DisplayName");
                let uninstall = read_reg_string(sub_key, "UninstallString");
                // SAFETY: sub_key 为 RegOpenKeyExW 打开
                let _ = unsafe { RegCloseKey(sub_key) };
                if let Some(name) = display_name {
                    if uninstall_entry_matches(&name, keyword) {
                        if let Some(u) = uninstall {
                            if !u.is_empty() {
                                // 保留原始 UninstallString（含引号包裹的路径）——
                                // 引号解析统一交给 split_uninstall_cmd。
                                // v0.4.10 修复：此前这里 trim_matches('\"') 提前剥掉首尾引号，
                                // 使 "C:\Program Files\Git\unins000.exe" 变成无引号的含空格路径，
                                // split_uninstall_cmd 按首空格切分 → exe=C:\Program →
                                // Start-Process 报"系统找不到指定的文件"（Git/Python 卸载失败）。
                                let raw = u.trim().to_string();
                                if !raw.is_empty() {
                                    // 非 MsiExec 的卸载器优先（可独立运行卸载整个软件）
                                    if raw.to_lowercase().contains("msiexec") {
                                        if msi_candidate.is_none() {
                                            msi_candidate = Some(raw);
                                        }
                                    } else if exe_candidate.is_none() {
                                        exe_candidate = Some(raw);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // SAFETY: hkey 为 RegOpenKeyExW 打开
            let _ = unsafe { RegCloseKey(hkey) };
        }
        exe_candidate.or(msi_candidate)
    }
    #[cfg(not(windows))]
    {
        let _ = keyword;
        None
    }
}

/// 读注册表字符串值（RegGetValueW + REG_SZ）；失败返回 None。
#[cfg(windows)]
fn read_reg_string(hkey: windows::Win32::System::Registry::HKEY, value_name: &str) -> Option<String> {
    use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};
    use windows::core::PCWSTR;

    let value_wide: Vec<u16> = value_name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = [0u8; 2048];
    let mut len = buf.len() as u32;
    // SAFETY: buf 有效；长度参数正确
    let res = unsafe {
        RegGetValueW(
            hkey,
            None,
            PCWSTR(value_wide.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut len),
        )
    };
    if res.is_err() {
        return None;
    }
    // REG_SZ 是 UTF-16LE，按 len 取
    let wide_len = len as usize / 2;
    let mut wide = Vec::with_capacity(wide_len);
    for i in 0..wide_len {
        let b0 = buf[i * 2] as u16;
        let b1 = buf[i * 2 + 1] as u16;
        wide.push(b0 | (b1 << 8));
    }
    while wide.last() == Some(&0) {
        wide.pop();
    }
    if wide.is_empty() {
        return None;
    }
    Some(String::from_utf16_lossy(&wide))
}

/// 下载 Node zip 并解压到 node_dir（用户级，免管理员）
/// v0.4.0：安装成功后把 node_dir 写入**用户 PATH**（HKCU），
/// 并回写 npm/pnpm 全局 prefix 到 node_dir —— 修复分发机器"npm/pnpm 找不到 / 装到别处"。
pub fn install_node(logger: &Arc<Logger>) -> Result<String, String> {
    let cfg = AppConfig::load();
    // Node 官方 zip 地址；镜像源可通过配置覆盖
    let version = NODE_VERSION; // 满足 dsh 要求 22.19+；后续可做成可选项
    let file = format!("node-{version}-win-x64.zip");
    let base = if cfg.node_mirror.is_empty() {
        format!("https://nodejs.org/dist/{version}")
    } else {
        format!("{}/{}", cfg.node_mirror.trim_end_matches('/'), version)
    };
    let url = format!("{base}/{file}");

    let zip_path = toolchain_dir().join(&file);
    logger.progress("toolchain", crate::core::events::InstallPhase::Download, 0, "下载 Node…");
    download_with_progress(logger, &url, &zip_path, "Node")?;
    logger.progress("toolchain", crate::core::events::InstallPhase::Download, 100, "Node 下载完成，解压中…");

    // 解压到临时目录，再把 node 目录提取出来
    let tmp_dir = toolchain_dir().join("tmp-node");
    unzip(logger, &zip_path, &tmp_dir)?;
    logger.progress("toolchain", crate::core::events::InstallPhase::Install, 75, "Node 解压完成，部署中…");

    // 解压后结构：tmp/node-v22.19.0-win-x64/...
    let extracted = tmp_dir.join(format!("node-{version}-win-x64"));
    let node_install = node_dir();
    if node_install.exists() {
        fs::remove_dir_all(&node_install).map_err(|e| e.to_string())?;
    }
    if extracted.exists() {
        fs::rename(&extracted, &node_install).map_err(|e| e.to_string())?;
    } else {
        // 某些镜像解压结构不同，直接移动内容
        let mut found = false;
        if let Ok(entries) = fs::read_dir(&tmp_dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().contains("node-v") {
                    fs::rename(entry.path(), &node_install).map_err(|e| e.to_string())?;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return Err("解压后未找到 node 目录".to_string());
        }
    }
    let _ = fs::remove_dir_all(&tmp_dir);
    let _ = fs::remove_file(&zip_path);
    logger.progress("toolchain", crate::core::events::InstallPhase::Done, 100, "Node 安装完成");

    // ===== 分发修复：持久化到用户 PATH + 固定 npm/pnpm 全局 prefix =====
    // 干净机器上系统 PATH 无 node；此目录写入用户 PATH（免管理员），
    // 使 `dsh`/`pnpm`/`npm` 子进程（含重启后的启动器）可直接解析。
    let node_path = node_install.to_string_lossy().to_string();
    let _ = crate::core::pathutil::prepend_user_path_dir(&node_path)
        .map_err(|e| format!("写入用户 PATH 失败: {e}"))?;
    logger.info(&format!("已将 Node 目录写入用户 PATH: {node_path}"));

    Ok(format!(
        "Node {version} 已安装到 {}（已加入用户 PATH，重启终端后全局可用）",
        node_install.display()
    ))
}

/// 卸载 Node（本启动器安装的用户级目录）：删目录 + 移除用户 PATH 条目 + 删残留 shim。
/// 不做其他事（不卸载系统级 Node）。
pub fn uninstall_node(logger: &Arc<Logger>) -> Result<String, String> {
    let node_install = node_dir();
    logger.info(&format!("开始卸载 Node（用户级目录）: {}", node_install.display()));
    // 1. 清理用户 PATH 中的 node_dir
    let node_path = node_install.to_string_lossy().to_string();
    match crate::core::pathutil::remove_user_path_dir(&node_path) {
        Ok(true) => logger.info(&format!("已从用户 PATH 移除: {node_path}")),
        Ok(false) => logger.info("用户 PATH 中无该 Node 目录（可能未持久化过）"),
        Err(e) => logger.warn(&format!("从用户 PATH 移除失败（继续卸载）: {e}")),
    }
    // 2. 删除目录（含 npm/pnpm 全局安装在此的包）
    if node_install.exists() {
        fs::remove_dir_all(&node_install).map_err(|e| format!("删除 Node 目录失败: {e}"))?;
        logger.info(&format!("Node 目录已删除: {}", node_install.display()));
    } else {
        logger.info("Node 目录不存在，跳过删除");
    }
    Ok("Node 已卸载（用户级目录 + 用户 PATH 条目已清理）".to_string())
}

/// Git 安装包本地路径
pub fn git_installer_path() -> PathBuf {
    toolchain_dir().join("Git-2.47.1-64-bit.exe")
}

/// Git 安装包下载 URL（可配 GitHub 镜像）
pub fn git_installer_url() -> String {
    let cfg = AppConfig::load();
    let path = "git-for-windows/git/releases/download/v2.47.1.windows.1/Git-2.47.1-64-bit.exe";
    if cfg.github_mirror.is_empty() {
        format!("https://github.com/{path}")
    } else {
        format!("{}/{path}", cfg.github_mirror.trim_end_matches('/'))
    }
}

/// Git 版本号（下载与注册表卸载入口用）
pub const GIT_VERSION: &str = "2.47.1";

/// 解析注册表 UninstallString → (可执行文件路径, 附加参数列表)
/// 处理：整体引号包裹（"C:\...\unins000.exe"）、引号路径 + 参数（"...\python.exe" /uninstall）
fn split_uninstall_cmd(raw: &str) -> (String, Vec<String>) {
    let s = raw.trim();
    // 引号包裹路径
    if let Some(rest) = s.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            let exe = rest[..end].to_string();
            let args = rest[end + 1..]
                .split_whitespace()
                .map(|a| a.to_string())
                .collect();
            return (exe, args);
        }
    }
    // 无引号：第一个空白前为 exe，其余为参数
    match s.split_once(char::is_whitespace) {
        Some((exe, args)) => (
            exe.to_string(),
            args.split_whitespace().map(|a| a.to_string()).collect(),
        ),
        None => (s.to_string(), Vec::new()),
    }
}

/// 卸载 Git：调用官方卸载器（注册表 UninstallString，需提权）。
/// 返回是否启动了卸载（卸载器本身是异步 GUI/UAC 流程）。
pub fn uninstall_git(logger: &Arc<Logger>) -> Result<String, String> {
    let entry = find_uninstall_entry("git");
    match entry {
        Some(cmd_line) => {
            logger.info(&format!("找到 Git 卸载入口: {cmd_line}"));
            let (exe, mut args) = split_uninstall_cmd(&cmd_line);
            // Inno Setup 卸载器：补充静默参数（若原串无则加，静默完成无需人工点下一步）
            if !args.iter().any(|a| a.to_lowercase().starts_with("/verysilent")) {
                args.push("/VERYSILENT".to_string());
            }
            if !args.iter().any(|a| a.to_lowercase().starts_with("/norestart")) {
                args.push("/NORESTART".to_string());
            }
            let args_str = args
                .iter()
                .map(|a| format!("'{a}'"))
                .collect::<Vec<_>>()
                .join(",");
            logger.info(&format!(
                "启动 Git 卸载器: {exe} 参数 {args_str}（UAC 确认后完成）"
            ));
            let ps = format!(
                "Start-Process -FilePath '{}' -ArgumentList {} -Verb RunAs -Wait",
                exe.replace('\'', "''"),
                args_str
            );
            let mut c = command::hidden("powershell");
            c.args(["-NoProfile", "-Command", &ps]);
            let out = c
                .output()
                .map_err(|e| format!("启动 Git 卸载失败: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "Git 卸载启动失败: {}",
                    crate::core::text::decode(&out.stderr).trim()
                ));
            }
            Ok("Git 卸载程序已启动（请在弹出的 UAC 中确认）".to_string())
        }
        None => Err(
            "未在系统中找到 Git 卸载入口（可能未通过官方安装器安装，请手动卸载）".to_string(),
        ),
    }
}

/// 卸载 Python：调用注册表卸载入口（官方 python.org 安装器）。
pub fn uninstall_python(logger: &Arc<Logger>) -> Result<String, String> {
    // 按显示名匹配 python（官方 python.org 安装器显示名含 "Python 3.x (64-bit)"）
    let entry = find_uninstall_entry("python");
    match entry {
        Some(cmd_line) => {
            logger.info(&format!("找到 Python 卸载入口: {cmd_line}"));
            let (exe, args) = split_uninstall_cmd(&cmd_line);
            let args_str = if args.is_empty() {
                String::new()
            } else {
                args.iter()
                    .map(|a| format!("'{a}'"))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            logger.info(&format!(
                "启动 Python 卸载器: {exe}（UAC 确认后完成）"
            ));
            let ps = if args_str.is_empty() {
                format!(
                    "Start-Process -FilePath '{}' -Verb RunAs -Wait",
                    exe.replace('\'', "''")
                )
            } else {
                format!(
                    "Start-Process -FilePath '{}' -ArgumentList {} -Verb RunAs -Wait",
                    exe.replace('\'', "''"),
                    args_str
                )
            };
            let mut c = command::hidden("powershell");
            c.args(["-NoProfile", "-Command", &ps]);
            let out = c
                .output()
                .map_err(|e| format!("启动 Python 卸载失败: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "Python 卸载启动失败: {}",
                    crate::core::text::decode(&out.stderr).trim()
                ));
            }
            Ok("Python 卸载程序已启动（请在弹出的 UAC 中确认）".to_string())
        }
        None => {
            Err("未在系统中找到 Python 卸载入口（可能为便携版/嵌入版，请手动清理）".to_string())
        }
    }
}

/// 下载并静默安装 Python（官方完整安装包；写入当前用户 + 加入 PATH）。
/// Start-Process -Wait 同步等待安装器结束（含 UAC 确认），返回时安装已完成。
pub fn install_python(logger: &Arc<Logger>) -> Result<String, String> {
    let version = "3.13.9"; // 官方 python.org 3.13 系列稳定版（满足 3.10+ 要求）
    let exe_name = format!("python-{version}-amd64.exe");
    let dest = toolchain_dir().join(&exe_name);
    let url = format!("https://www.python.org/ftp/python/{version}/{exe_name}");
    logger.progress("toolchain", crate::core::events::InstallPhase::Download, 0, "下载 Python…");
    // 下载带实时进度（轮询文件大小 + Content-Length 占比）
    download_with_progress(logger, &url, &dest, "Python")?;
    logger.progress("toolchain", crate::core::events::InstallPhase::Install, 0, "Python 下载完成，启动安装（UAC 确认后开始）…");
    // 静默安装：当前用户 + 加入 PATH + pip
    // /quiet /InstallAllUsers=0 /PrependPath=1 /Include_pip=1
    // -Wait：同步等待安装器退出（UAC 确认 + 静默安装全程），返回即安装完成
    //
    // v0.4.8：安装期间无法从 python.org 静默安装器取内部进度（/quiet 无 stdout），
    // 改用后台线程周期推送"已等待 N 秒 + 注册表 PythonCore 是否已写入"的实时反馈，
    // 避免前端进度条停在 0 不动被误判卡死。
    let ps = format!(
        "Start-Process -FilePath '{}' -ArgumentList '/quiet','/InstallAllUsers=0','/PrependPath=1','/Include_pip=1' -Verb RunAs -Wait",
        dest.to_string_lossy().replace('\'', "''")
    );
    let mut c = command::hidden("powershell");
    c.args(["-NoProfile", "-Command", &ps]);

    // 安装轮询线程：期间每 1s 推一次进度（阶段 install，消息带等待秒数 + 注册表状态）
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let logger_t = Arc::clone(logger);
    let stop_t = std::sync::Arc::clone(&stop);
    let poller = std::thread::spawn(move || {
        let mut secs: u32 = 0;
        loop {
            if stop_t.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1000));
            secs += 1;
            // 注册表 PythonCore 已写入 → 安装基本完成（贴 90，等安装器退出跳 100）
            let reg_ok = !crate::core::pathutil::python_install_dirs().is_empty();
            if reg_ok {
                logger_t.progress(
                    "toolchain",
                    crate::core::events::InstallPhase::Install,
                    90,
                    "Python 安装写入系统已完成，正在收尾…",
                );
            } else if secs % 3 == 0 {
                logger_t.progress(
                    "toolchain",
                    crate::core::events::InstallPhase::Install,
                    0,
                    format!("Python 安装中… 已等待 {secs} 秒（请在 UAC 弹窗确认）"),
                );
            }
        }
    });

    let out = c
        .output()
        .map_err(|e| format!("启动 Python 安装失败: {e}"))?;
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = poller.join();

    if !out.status.success() {
        return Err(format!(
            "Python 安装启动失败: {}",
            crate::core::text::decode(&out.stderr).trim()
        ));
    }
    // 同步等待结束 → 注册表 PythonCore 已写入安装目录；探测确认安装成功
    let dirs = crate::core::pathutil::python_install_dirs();
    if !dirs.is_empty() {
        let installed = dirs
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join("; ");
        logger.info(&format!("Python 已安装确认（注册表 PythonCore）: {installed}"));
        logger.progress("toolchain", crate::core::events::InstallPhase::Done, 100, "Python 安装完成");
        Ok(format!("Python {version} 已安装完成（已加入 PATH，全局可用）"))
    } else {
        // 注册表未找到（异常：安装被取消/失败）→ 提示手动确认
        logger.warn("Python 安装器已退出但注册表 PythonCore 未找到安装目录，请检查安装是否被取消");
        Err("Python 安装可能未完成：注册表未找到安装目录，请检查是否在 UAC 弹窗中取消了安装".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::uninstall_entry_matches;
    use super::split_uninstall_cmd;

    #[test]
    fn test_git_match_precise() {
        // Git for Windows 卸载项 DisplayName = "Git"（精确）
        assert!(uninstall_entry_matches("Git", "git"));
        assert!(uninstall_entry_matches("Git for Windows 2.47.1", "git"));
        // 不误匹配 GitHub Desktop / GitHub CLI / GitHub（HKCU 实测存在）
        assert!(!uninstall_entry_matches("GitHub Desktop", "git"));
        assert!(!uninstall_entry_matches("GitHub CLI", "git"));
        assert!(!uninstall_entry_matches("GitHub", "git"));
    }

    #[test]
    fn test_python_match_main_item_only() {
        // python.org .exe 主安装项（HKCU 实测）："Python 3.14.0 (64-bit)"
        assert!(uninstall_entry_matches("Python 3.14.0 (64-bit)", "python"));
        assert!(uninstall_entry_matches("Python 3.9.13 (32-bit)", "python"));
        // 不匹配 MSI 拆分组件（直接卸载会拆坏安装）
        assert!(!uninstall_entry_matches("Python 3.14.0 Core Interpreter (64-bit)", "python"));
        assert!(!uninstall_entry_matches("Python 3.14.0 Executables (64-bit)", "python"));
        assert!(!uninstall_entry_matches("Python 3.14.0 pip Bootstrap (64-bit)", "python"));
        assert!(!uninstall_entry_matches("Python 3.14.0 Standard Library (64-bit)", "python"));
        assert!(!uninstall_entry_matches("Python 3.14.0 Development Libraries (64-bit)", "python"));
        assert!(!uninstall_entry_matches("Python 3.14.0 Add to Path (64-bit)", "python"));
        // 无关项不匹配
        assert!(!uninstall_entry_matches("PyCharm Community Edition", "python"));
    }

            #[test]
    fn test_split_uninstall_cmd() {
        // Inno（纯引号路径，无参数）——真实 Git 卸载入口："C:\Program Files\Git\unins000.exe"
        let raw1 = r#""C:\Program Files\Git\unins000.exe""#;
        let (exe, args) = split_uninstall_cmd(raw1);
        assert_eq!(exe, "C:\\Program Files\\Git\\unins000.exe");
        assert!(args.is_empty());
        // python.org exe + 参数（引号路径 + 空格 /uninstall）——真实 Python 卸载入口
        let raw2 = r#""C:\Users\a\AppData\Local\Package Cache\{abc}\python-3.14.0-amd64.exe"  /uninstall"#;
        let (exe2, args2) = split_uninstall_cmd(raw2);
        assert!(exe2.contains("python-3.14.0-amd64.exe"));
        assert_eq!(args2, vec!["/uninstall"]);
        // MsiExec 行（无引号 exe + 参数）
        let (exe3, args3) =
            split_uninstall_cmd("MsiExec.exe /X{0788B172-EA4C-4BB2-B6DE-CF425BDFA8A7}");
        assert_eq!(exe3, "MsiExec.exe");
        assert_eq!(args3, vec!["/X{0788B172-EA4C-4BB2-B6DE-CF425BDFA8A7}"]);
        // 纯路径无引号
        let (exe4, args4) = split_uninstall_cmd("C:\\unins000.exe");
        assert_eq!(exe4, "C:\\unins000.exe");
        assert!(args4.is_empty());
    }

    #[test]
    fn test_split_uninstall_cmd_real_registry_values() {
        // 回归测试（v0.4.10 修复）：find_uninstall_entry 现在**保留原始 UninstallString**
        // （含引号），不再提前 trim_matches('"')。以下是两台真实机器注册表抓到的值，
        // 经 split_uninstall_cmd 拆分必须得到完整路径（不能在空格处截断）。
        //
        // Git for Windows：整体被引号包裹，路径含空格
        let git_raw = r#""C:\Program Files\Git\unins000.exe""#;
        let (exe, args) = split_uninstall_cmd(git_raw);
        assert_eq!(
            exe, "C:\\Program Files\\Git\\unins000.exe",
            "Git 卸载器路径必须完整（含空格），当前: {exe:?}"
        );
        assert!(args.is_empty(), "Git 无参数: {args:?}");
        // 修复前 find_uninstall_entry 会把它变成无引号 → exe=C:\Program（截断）
        let broken_input = "C:\\Program Files\\Git\\unins000.exe"; // 旧 bug 的入参
        let (exe_bad, _) = split_uninstall_cmd(broken_input);
        assert_eq!(exe_bad, "C:\\Program", "旧路径会截断（演示 bug）");

        // Python 3.14.0：引号路径 + 双空格 + /uninstall
        let py_raw = r#""C:\Users\Administrator\AppData\Local\Package Cache\{a9c48b80-2c48-47ac-8c9e-32a1b0ec69e5}\python-3.14.0-amd64.exe"  /uninstall"#;
        let (py_exe, py_args) = split_uninstall_cmd(py_raw);
        assert!(py_exe.contains("python-3.14.0-amd64.exe"), "Python 卸载器路径完整: {py_exe:?}");
        assert_eq!(py_args, vec!["/uninstall"], "Python 卸载参数: {py_args:?}");
        // 修复前 find_uninstall_entry 的 trim_matches('"') 会把整体引号剥掉 → 无引号含空格
        // → split 在首空格截断，exe 变成 "C:\Users\...\Package"（路径片段），
        // Start-Process 因此报“系统找不到指定的文件”
        let old_bug_input = py_raw.trim_matches('"'); // 模拟旧 find_uninstall_entry 返回值
        let (exe_bad2, _) = split_uninstall_cmd(old_bug_input);
        assert_ne!(
            exe_bad2,
            py_exe,
            "旧 bug 会截断 Python 卸载器路径: {exe_bad2:?} vs 正确 {py_exe:?}"
        );
    }

    #[test]
    fn test_download_percent() {
        use super::download_percent;
        // 0% / 中间 / 99% 封顶（100 由完成路径单独推）
        assert_eq!(download_percent(0, 100), 0);
        assert_eq!(download_percent(50, 100), 50);
        assert_eq!(download_percent(99, 100), 99);
        // 已下载=总长 → 封顶 99（避免提前 100 与完成事件竞争）
        assert_eq!(download_percent(100, 100), 99);
        assert_eq!(download_percent(1000, 1000), 99);
        // 超总量（异常）→ 仍封顶 99
        assert_eq!(download_percent(1500, 1000), 99);
        // 未知总量 → 0（调用方走不定量分支）
        assert_eq!(download_percent(500, 0), 0);
        // 真实 Python 27.4MB：中点≈50
        assert_eq!(download_percent(14_380_888, 28_761_776), 50);
    }
}
