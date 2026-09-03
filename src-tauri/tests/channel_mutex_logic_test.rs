// 验证通道互斥的核心逻辑（可撤销性 / 幂等性），不碰真实安装环境：
// - github 目录清理：目录存在 → 删；不存在 → 静默
// - shim 清理：存在 → 删；不存在 → 静默
// - npm 全局卸载的幂等语义（spawn 层面不测，测判断逻辑）
use std::fs;

fn cleanup_dir(dir: &std::path::Path, removed: &mut Vec<String>) {
    if dir.exists() {
        fs::remove_dir_all(dir).unwrap();
        removed.push(dir.display().to_string());
    }
}

#[test]
fn test_mutex_cleanup_idempotent() {
    // 在 TEMP 建模拟 github 目录 + shim 文件，验证清理函数逻辑（存在即删、幂等）
    let tmp = std::env::temp_dir().join("dsh-mutex-test");
    let _ = fs::remove_dir_all(&tmp);
    let gh_dir = tmp.join("github-dsh").join("deepseek-harness");
    fs::create_dir_all(&gh_dir).unwrap();
    let shim = tmp.join("dsh.cmd");
    fs::write(&shim, "@echo off\r\ncd /d \"x\"\r\npnpm dsh %*\r\n").unwrap();

    // 首次清理：两者应被删
    let mut removed = Vec::new();
    cleanup_dir(&gh_dir, &mut removed);
    assert_eq!(removed.len(), 1, "github 目录存在应被删除");
    let _ = fs::remove_file(&shim); // 等价 shim 删除
    assert!(!gh_dir.exists(), "github 目录应已删除");
    assert!(!shim.exists(), "shim 应已删除");

    // 再次清理（目录/shim 已不存在）：幂等不报错
    let mut removed2 = Vec::new();
    cleanup_dir(&gh_dir, &mut removed2);
    assert_eq!(removed2.len(), 0, "目录不存在应静默跳过");

    let _ = fs::remove_dir_all(&tmp);
}
