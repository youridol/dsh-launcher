//! v0.9.9 端到端回归：环境类陈旧 lastError 在 dsh 恢复可用后由 list() 清除。
//! 用真实 profile + 真实注册表（测试后还原）。

use dsh_launcher_lib::core::logging::Logger;
use dsh_launcher_lib::core::plugin;
use std::sync::Arc;

#[test]
fn stale_environment_error_cleared_on_list() {
    if std::env::var("DSH_LAUNCHER_E2E").is_err() {
        eprintln!("skip: DSH_LAUNCHER_E2E 未设置（需要真实 dsh 环境）");
        return;
    }
    let registry_path = plugin::registry::Registry::path();
    let original = std::fs::read_to_string(&registry_path).ok();

    // 1) 注入陈旧环境类错误（复刻 2026-09-16 10:21 现场的两条记录）
    let mut registry = plugin::registry::Registry::load("web");
    let targets = ["dshmarket", "@xmanrui/dsh-im"];
    let mut injected = 0usize;
    for target in targets {
        if let Some(record) = registry.plugins.iter_mut().find(|r| r.package == target) {
            record.last_error = Some(
                "dsh 安装目录缺失（GitHub shim 指向的目录已不存在或为空），请重新安装 dsh"
                    .to_string(),
            );
            injected += 1;
        }
    }
    assert!(injected > 0, "真实注册表应含目标插件（dshmarket / @xmanrui/dsh-im）");
    registry.save().unwrap();

    // 2) list()（dsh 当前可用）应清除陈旧环境错误
    let logger = Arc::new(Logger::init());
    let result = plugin::list("web", &logger).expect("list 应成功（dsh 可用）");
    for item in &result.plugins {
        if targets.contains(&item.package.as_str()) {
            assert!(
                item.last_error.is_none(),
                "{} 的陈旧环境错误应被清除，实际 {:?}",
                item.package,
                item.last_error
            );
        }
    }
    // 3) 落盘确认
    let after = plugin::registry::Registry::load("web");
    for record in after.plugins.iter().filter(|r| targets.contains(&r.package.as_str())) {
        assert!(record.last_error.is_none(), "{} 未落盘清除", record.package);
    }

    // 4) 还原注册表
    let restore = match original {
        Some(text) => std::fs::write(&registry_path, text),
        None => std::fs::remove_file(&registry_path),
    };
    restore.unwrap();
}

/// UI 等价路径：连续两次 list（第二次读第一次落盘的结果），
/// 保证面板最终稳定显示无陈旧错误。
#[test]
fn list_is_idempotent_after_clearing() {
    if std::env::var("DSH_LAUNCHER_E2E").is_err() {
        eprintln!("skip");
        return;
    }
    let logger = Arc::new(Logger::init());
    let first = plugin::list("web", &logger).expect("first list");
    let second = plugin::list("web", &logger).expect("second list");
    for item in &second.plugins {
        assert!(
            item.last_error.is_none()
                || !plugin::is_environment_error(item.last_error.as_deref().unwrap_or("")),
            "{} 第二次 list 仍有环境类错误: {:?}",
            item.package,
            item.last_error
        );
    }
    let _ = first;
}
