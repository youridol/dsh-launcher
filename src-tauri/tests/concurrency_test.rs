// 并发测试：验证命令 async + spawn_blocking 后，多个长任务互不阻塞
use std::time::{Duration, Instant};

/// 模拟一个耗时命令（如 npm view 网络请求）
fn slow_cmd(name: &str, ms: u64) -> String {
    std::thread::sleep(Duration::from_millis(ms));
    format!("{name} done")
}

#[test]
fn test_spawn_blocking_concurrent() {
    // 用 tokio 模拟 Tauri async runtime 的并发行为
    let rt = tokio::runtime::Runtime::new().unwrap();
    let start = Instant::now();
    rt.block_on(async {
        // 3 个并发长任务（各 300ms）——若串行执行需 900ms，并发约 300ms
        let (a, b, c) = tokio::join!(
            tokio::task::spawn_blocking(|| slow_cmd("a", 300)),
            tokio::task::spawn_blocking(|| slow_cmd("b", 300)),
            tokio::task::spawn_blocking(|| slow_cmd("c", 300)),
        );
        assert_eq!(a.unwrap(), "a done");
        assert_eq!(b.unwrap(), "b done");
        assert_eq!(c.unwrap(), "c done");
    });
    let elapsed = start.elapsed();
    println!("并发 3×300ms 任务总耗时: {:?}（应≈300ms，串行会是900ms）", elapsed);
    // 并发应显著小于串行（900ms）；容忍调度开销给 <700ms
    assert!(elapsed < Duration::from_millis(700), "未并发执行! 耗时 {elapsed:?}");
}
