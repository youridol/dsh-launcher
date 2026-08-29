// 集成测试：验证修复后的 npm 命令执行（hidden_cmd 方案）能正确列出版本
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 模拟 command::hidden_cmd("npm")
fn npm_cmd() -> Command {
    let mut c = Command::new("cmd.exe");
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c.arg("/D").arg("/C").arg("npm");
    c
}

#[test]
fn test_npm_list_versions_full() {
    // 复刻 commands/version.rs list_npm_versions 逻辑
    let mut c = npm_cmd();
    c.args(["view", "@deepseek-ai/dsh", "versions", "--json"]);
    let out = c.output().expect("npm view 执行失败");
    assert!(out.status.success(), "npm view 非零退出: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    let versions: Vec<String> = serde_json::from_str(&text).expect("解析版本列表失败");
    assert!(!versions.is_empty(), "版本列表为空");
    println!("✅ 版本列表获取成功，共 {} 个版本", versions.len());
    println!("   最新: {:?}", &versions[versions.len().saturating_sub(5)..]);
}

#[test]
fn test_npm_install_dry() {
    // 验证 npm install 命令能正确执行（用 --dry-run 避免真装）
    let mut c = npm_cmd();
    c.args(["install", "-g", "--dry-run", "@deepseek-ai/dsh@0.1.1-rc.2"]);
    let out = c.output().expect("npm install 执行失败");
    assert!(out.status.success(), "npm install 非零退出: {}", String::from_utf8_lossy(&out.stderr));
    println!("✅ npm install 命令可正常执行（dry-run）");
}
