//! 影子恢复行（v0.9.7）端到端回归：真实 dsh + 真实 profile。
//!
//! 场景（2026-09-16 故障）：@michengai/dsh-archive-manager 的 bundle patch
//! 禁用官方 web-app 的 `workspace` 行并 insert 子类。对插件执行 set_state(disable)
//! 时，受管区块必须同时写入官方 `workspace` 行的启用行（shadow restore），
//! 否则 workspaceRegistry 无人提供 → 5 个下游插件 pending → Sessions/工作区不可访问。
//!
//! 前置：本机已安装 dsh（GitHub 通道或 npm）且 profile web 已装
//! @michengai/dsh-archive-manager。环境无 dsh 时跳过（CI 语义）。

use dsh_launcher_lib::core::logging::Logger;
use dsh_launcher_lib::core::plugin::{self, dump, managed};
use std::sync::Arc;

fn have_dsh() -> bool {
    std::env::var("DSH_LAUNCHER_E2E").is_ok()
}

fn registry_shadow_of(package: &str) -> Vec<String> {
    let registry = dsh_launcher_lib::core::plugin::registry::Registry::load("web");
    registry
        .plugins
        .iter()
        .find(|record| record.package == package)
        .map(|record| record.shadow_restored.clone())
        .unwrap_or_default()
}

#[test]
fn shadow_restore_end_to_end() {
    if !have_dsh() {
        eprintln!("skip: DSH_LAUNCHER_E2E 未设置（需要真实 dsh 环境）");
        return;
    }
    let logger = Arc::new(Logger::init());
    let patch_path = dsh_launcher_lib::core::dshhome::profile_patch_path("web").unwrap();
    let before = managed::read_block(&patch_path).unwrap();

    // 1) disable archive-manager → 官方 workspace 行应被影子恢复
    let result = plugin::set_state("web", "@michengai/dsh-archive-manager", false, &logger);
    assert!(result.is_ok(), "disable 失败: {result:?}");

    let after_disable = managed::read_block(&patch_path).unwrap().unwrap_or_default();
    let ws_entry = after_disable.iter().find(|entry| entry.id == "workspace");
    assert!(
        ws_entry.map(|entry| entry.disabled == false).unwrap_or(false),
        "受管区块应含 workspace 行的启用条目，实际: {after_disable:?}"
    );

    // 2) dump 复核：官方 workspace 行已恢复启用（disabled: true 被受管启用行覆盖）
    let text = dsh_launcher_lib::core::profile::dump_config("web").unwrap();
    let sections = dump::parse_dump(&text).unwrap();
    let index = dump::index_by_id(&sections);
    let (_, ws_row) = index.get("workspace").expect("dump 缺 workspace 行");
    assert_eq!(
        ws_row.effective_enabled(),
        Some(true),
        "官方 workspace 行未被影子恢复"
    );

    // 3) enable 回去 → 影子启用行应被移除，回到插件的替换状态
    let result = plugin::set_state("web", "@michengai/dsh-archive-manager", true, &logger);
    assert!(result.is_ok(), "enable 失败: {result:?}");
    let after_enable = managed::read_block(&patch_path).unwrap().unwrap_or_default();
    assert!(
        after_enable.iter().all(|entry| entry.id != "workspace"),
        "enable 后影子启用行应被移除，实际: {after_enable:?}"
    );

    // 3b) v0.9.8 真实回归：连续两次 disable（幂等路径）不得清空影子记忆 ——
    // 序列 disable → disable → enable 必须仍然移除影子行。
    let r1 = plugin::set_state("web", "@michengai/dsh-archive-manager", false, &logger);
    assert!(r1.is_ok(), "重复 disable 失败: {r1:?}");
    // 连续第二次 disable（真实触发序列：dsh live 重载/repair 收敛会重放期望态）
    let r1b = plugin::set_state("web", "@michengai/dsh-archive-manager", false, &logger);
    assert!(r1b.is_ok(), "连续 disable 失败: {r1b:?}");
    assert!(
        !registry_shadow_of("@michengai/dsh-archive-manager").is_empty(),
        "连续 disable 后注册表影子记忆被清空（BUG）：{:?}",
        registry_shadow_of("@michengai/dsh-archive-manager")
    );
    let after_repeat_disable = managed::read_block(&patch_path).unwrap().unwrap_or_default();
    assert!(
        after_repeat_disable.iter().any(|e| e.id == "workspace"),
        "重复 disable 后影子行仍在受管区块，实际: {after_repeat_disable:?}"
    );
    let r2 = plugin::set_state("web", "@michengai/dsh-archive-manager", true, &logger);
    assert!(r2.is_ok(), "重复 disable 后的 enable 失败: {r2:?}");
    let after_repeat_enable = managed::read_block(&patch_path).unwrap().unwrap_or_default();
    assert!(
        after_repeat_enable.iter().all(|e| e.id != "workspace"),
        "重复 disable 后再 enable 应移除影子行（记忆被清空的 BUG），实际: {after_repeat_enable:?}"
    );

    // 4) 现场还原：受管区块回到测试前状态
    let restore = match before {
        Some(entries) => managed::apply_block(&patch_path, &entries),
        None => managed::apply_block(&patch_path, &[]),
    };
    restore.unwrap();
}
