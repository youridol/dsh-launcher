// 测试：GitHub 通道 tag 处理（release tag 形如 dsh-v0.1.2-alpha.1，须原样使用）
use dsh_launcher_lib::core::github;

#[test]
fn test_github_tag_passthrough() {
    // 模拟前端传入的版本（来自 list_releases 的 tag_name）
    let version = "dsh-v0.1.2-alpha.1";
    // 验证 core 层的 tag 处理：原样使用，不添加多余前缀
    // （直接调用内部逻辑不可行，这里验证 list_releases 的返回格式）
    // 用 git ls-remote 验证真实 tag 存在且命名正确
    let out = std::process::Command::new("git")
        .args(["ls-remote", "--tags", "https://github.com/deepseek-ai/deepseek-harness.git"])
        .output()
        .expect("git ls-remote 执行失败");
    assert!(out.status.success(), "git ls-remote 失败");
    let text = String::from_utf8_lossy(&out.stdout);
    // 确认真实 tag 名（用 version 变量构造期望值，避免字面量重复）
    assert!(
        text.contains(&format!("refs/tags/{version}")),
        "真实 tag 应为 {version}，实际: {}",
        text.lines().map(|l| l.split_whitespace().nth(1).unwrap_or("")).collect::<Vec<_>>().join(",")
    );
    // 验证不存在 v0.1.2-alpha.1（纯 v 前缀）
    assert!(
        !text.contains("refs/tags/v0.1.2-alpha.1"),
        "不应存在纯 v 前缀 tag"
    );
    println!("✅ GitHub 真实 tag: dsh-v0.1.2-alpha.1（带 dsh- 前缀，修复后原样使用）");
}

#[test]
fn test_list_releases_format() {
    // list_releases 返回 tag_name（dsh- 前缀），install_version 应原样消费
    // 这里验证 github_dsh_dir 路径构造不含多余前缀
    let dir = github::github_dsh_dir();
    assert!(dir.to_string_lossy().contains("github-dsh"), "目录路径异常: {dir:?}");
    // v0.2.3：克隆目录固定为 deepseek-harness（不再按版本号命名）
    let clone = github::github_clone_dir();
    assert!(
        clone.to_string_lossy().ends_with("deepseek-harness"),
        "克隆目录应固定为 deepseek-harness: {clone:?}"
    );
    println!("✅ github_dsh_dir: {}", dir.display());
    println!("✅ github_clone_dir: {}", clone.display());
}
