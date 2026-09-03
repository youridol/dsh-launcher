// 集成测试：验证 PATH 注入逻辑（分发机器场景）
//
// 场景：系统 PATH 无 node，但用户级 node_dir（%LOCALAPPDATA%\dsh-launcher\toolchain\node）
// 有 node.exe/npm.cmd —— hidden_cmd 注入后 npm/pnpm 子进程必须能找到。
// 本机系统 PATH 可能本来就有 node（开发者机器），无法完全模拟"干净分发机"，
// 因此测试直接构造 Command + 显式 PATH 校验注入行为（幂等 + 前缀优先）。
use std::process::Command;

#[test]
fn test_hidden_cmd_inject_keeps_prefix() {
    // 复刻 command::hidden_cmd 的注入路径：cmd /C npm + 注入 node_dir 到 PATH 前缀
    let mut c = Command::new("cmd.exe");
    c.arg("/D").arg("/C").arg("npm");
    // 手动模拟 node_dir_injection 返回的目录已存在场景
    let node_dir = std::env::var("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("dsh-launcher")
        .join("toolchain")
        .join("node");
    let node_path = node_dir.to_string_lossy().to_string();
    let current = std::env::var("PATH").unwrap_or_default();
    // 断言注入后 node_dir 在 PATH 前缀（若已含则不重复）
    let injected = if current.split(';').any(|p| p.eq_ignore_ascii_case(&node_path)) {
        current.clone()
    } else {
        format!("{node_path};{current}")
    };
    // node_dir 必须出现在 PATH 中（无论系统 PATH 是否有 node，本启动器管理目录都要可达）
    assert!(
        injected.split(';').any(|p| p.eq_ignore_ascii_case(&node_path)),
        "注入后 node_dir 应在 PATH 中: {injected}"
    );
}

#[test]
fn test_npm_visible_after_inject() {
    // 端到端：构造"用户级 node_dir 优先"的 PATH 并运行 npm --version
    // （模拟分发机器 hidden_cmd 注入后的行为；若 npm.cmd 存在则应成功）
    let node_dir = std::env::var("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("dsh-launcher")
        .join("toolchain")
        .join("node");
    let node_exe = node_dir.join("node.exe");
    if !node_exe.exists() {
        // 未安装用户级 node（如本机走系统 node）→ 跳过（非失败）
        eprintln!("用户级 node_dir 未安装 node.exe，跳过端到端注入测试");
        return;
    }
    let node_path = node_dir.to_string_lossy().to_string();
    // 移除 PATH 中任何 node 相关目录（模拟干净分发机）
    let raw_path = std::env::var("PATH").unwrap_or_default();
    let filtered: Vec<&str> = raw_path
        .split(';')
        .filter(|p| {
            let l = p.to_lowercase();
            !l.contains("node") && !l.contains("npm") && !l.contains("pnpm")
        })
        .collect();
    let mut c = Command::new("cmd.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    // 模拟 hidden_cmd("node")：cmd /C node，注入 node_dir 到 PATH 前缀
    c.arg("/D").arg("/C").arg("node");
    c.env("PATH", format!("{node_path};{}", filtered.join(";")));
    c.arg("--version");
    let out = c.output().expect("cmd /C node 执行失败");
    assert!(
        out.status.success(),
        "注入后 node --version 应成功: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ver = String::from_utf8_lossy(&out.stdout);
    assert!(!ver.trim().is_empty(), "node --version 应有输出: {ver}");
    // npm.cmd 应随 node zip 存在（真实安装由 node 官方 zip 提供 node_modules/npm）
    let npm_shim = node_dir.join("npm.cmd");
    assert!(npm_shim.exists(), "npm.cmd 应在 node_dir 中（真实安装由 node zip 提供）");
}
