//! v0.9.8 端到端回归：真实损坏的 profile patch → inspect 识别 → heal 自愈 →
//! dsh --dump-config 恢复成功。需要真实 dsh 环境（DSH_LAUNCHER_E2E=1）。
//!
//! 注意：本测试直接操作用户真实 profile 文件，测试前后均做备份/还原。

use dsh_launcher_lib::core::plugin;
use std::sync::Mutex;

/// 两个测试都操作真实 profile 文件：同二进制内串行执行，避免互相污染。
static PROFILE_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn heal_corrupt_patch_end_to_end() {
    if std::env::var("DSH_LAUNCHER_E2E").is_err() {
        eprintln!("skip: DSH_LAUNCHER_E2E 未设置（需要真实 dsh 环境）");
        return;
    }
    let _guard = PROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = dsh_launcher_lib::core::dshhome::profile_patch_path("web").unwrap();
    let original = std::fs::read_to_string(&path).unwrap();

    // 1) 放入真实损坏样本（2026-09-16 实测：模板头切 33 字节后的重复拼接）
    let template = "# Your patch layer for this dsh profile, applied after every bundle layer:\n\
# a top-level YAML array of loader patch entries (id-targeted config\n\
# overrides, disables, and insert lists; `!!js` expressions allowed).\n\
# []\n\
- id: mnemon\n\
  disabled: false\n";
    let corrupt = format!(
        "{template}# >>> dsh-launcher managed v1 — 由启动器维护，请勿手工编辑 >>>\n\
# <<< dsh-launcher managed v1 <<<\n{}",
        &template[33..]
    );
    std::fs::write(&path, &corrupt).unwrap();

    // 2) inspect 必须识别
    let reason = plugin::inspect_patch_file(&path).expect("损坏文件应被识别");
    assert!(
        reason.contains("模板头重复") || reason.contains("顶层不是"),
        "reason = {reason}"
    );

    // 3) heal 自愈
    let message = plugin::heal_patch_file(&path).expect("自愈应成功");
    assert!(message.contains("已修复"), "message = {message}");

    // 4) 自愈后结构正常 + 用户行保留
    assert!(plugin::inspect_patch_file(&path).is_none(), "自愈后结构应正常");
    let healed = std::fs::read_to_string(&path).unwrap();
    assert_eq!(healed.matches("profile, applied").count(), 1, "模板头应恰 1 份");
    assert!(healed.contains("- id: mnemon"), "用户行（mnemon）应保留:\n{healed}");

    // 5) 真实 dsh 能解析（dump-config 成功）
    let dump = dsh_launcher_lib::core::profile::dump_config("web");
    assert!(dump.is_ok(), "自愈后 dsh --dump-config 应成功: {dump:?}");

    // 6) 还原现场
    std::fs::write(&path, &original).unwrap();
}

/// BUG-3 回归：dsh 因 patch 文件损坏而启动失败时，归因必须给出
/// 「配置文件损坏」精确指引，而不是「逐个禁用插件」。
/// 直接用真实失败样本（2026-09-16 实测 stderr 片段）驱动 handle_boot_failure。
#[test]
fn boot_failure_attribution_prefers_config_diagnosis() {
    if std::env::var("DSH_LAUNCHER_E2E").is_err() {
        eprintln!("skip: DSH_LAUNCHER_E2E 未设置（需要真实 dsh 环境）");
        return;
    }
    let _guard = PROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = dsh_launcher_lib::core::dshhome::profile_patch_path("web").unwrap();
    let original = std::fs::read_to_string(&path).unwrap();
    let stderr_path = dsh_launcher_lib::core::process::dsh_stderr_path();
    let stderr_original = std::fs::read_to_string(&stderr_path).unwrap_or_default();

    // 注入损坏 + 真实失败 stderr（字段与实测一致）
    let corrupt = format!("{original}rofile, applied after every bundle layer:
");
    std::fs::write(&path, &corrupt).unwrap();
    let real_failure = "Error: dsh: failed to parse overlay C:\\Users\\Administrator\\.dsh\\profiles\\web\\cordis.patch.yml: YAMLException: end of the stream or a document separator is expected (9:1)\n";
    std::fs::write(&stderr_path, real_failure).unwrap();

    let logger = std::sync::Arc::new(dsh_launcher_lib::core::logging::Logger::init());
    let matched = dsh_launcher_lib::core::plugin::handle_boot_failure(&logger);
    assert!(
        matched.is_none(),
        "配置损坏时不得归因到具体插件（会误导用户逐个禁用），实际 matched={matched:?}"
    );

    // 还原
    std::fs::write(&path, &original).unwrap();
    std::fs::write(&stderr_path, &stderr_original).unwrap();
}

