// 测试：配置默认值（端口必须 3080，不允许 0）
use dsh_launcher_lib::core::config::AppConfig;

#[test]
fn test_default_port_is_3080() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.port, 3080, "默认端口必须是 3080，当前是 {}", cfg.port);
    assert!((1..=65535).contains(&cfg.port), "端口必须在合法范围");
}

#[test]
fn test_default_switches() {
    let cfg = AppConfig::default();
    // 默认：最小化到托盘开、保留 DSH_HOME 开、自动开浏览器开；其余关
    assert!(cfg.minimize_to_tray);
    assert!(cfg.keep_dsh_home_on_uninstall);
    assert!(cfg.auto_open_browser);
    assert!(!cfg.close_exits);
    assert!(!cfg.keep_dsh_on_exit);
    assert!(!cfg.auto_start_dsh);
}
