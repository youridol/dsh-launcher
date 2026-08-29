// 测试：托盘图标能被 tauri Image::from_bytes 解析（运行时托盘图标可加载）
#[test]
fn test_tray_icon_png_valid() {
    // 与 core/tray.rs 相同的嵌入路径
    let bytes = include_bytes!("../icons/32x32.png");
    assert!(!bytes.is_empty(), "图标字节为空");
    // PNG 魔数校验
    assert_eq!(&bytes[0..8], &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A], "不是合法 PNG");
    // 尺寸：IHDR 宽高
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    assert_eq!(w, 32, "宽度应为 32");
    assert_eq!(h, 32, "高度应为 32");
    println!("✅ 托盘图标 PNG 有效: {w}x{h}");
}

#[test]
fn test_tray_icon_parsable_by_tauri_image() {
    // 用 tauri 的 Image::from_bytes 验证运行时解析（image-png feature 已启用）
    let bytes = include_bytes!("../icons/32x32.png");
    let img = tauri::image::Image::from_bytes(bytes);
    assert!(img.is_ok(), "tauri Image 解析失败: {:?}", img.err());
    let img = img.unwrap();
    assert_eq!(img.width(), 32);
    assert_eq!(img.height(), 32);
    println!("✅ tauri Image 解析成功: {}x{}", img.width(), img.height());
}
