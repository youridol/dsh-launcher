//! 窗口图标端到端回归测试：创建真实隐藏窗口 → 设置 ICON_BIG（256px）→ 读回验证
//!
//! 背景：任务栏模糊根因 = ICON_BIG 从未成功设置（旧实现 CreateIconFromResourceEx
//! 对本项目 PNG 压缩 ICO 全部返回 ERROR_INVALID_HANDLE）。本测试验证新路径
//! （RGBA → CreateIcon 256px → WM_SETICON）在真实窗口上生效且保留原生分辨率。
//! 运行：cargo test --test icon_window_test -- --nocapture

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIcon, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow, GetIconInfo,
    RegisterClassW, SendMessageW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, ICONINFO, ICON_BIG,
    ICON_SMALL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_GETICON, WM_SETICON, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

/// RGBA → CreateIcon（tao from_rgba 同款：BGRA + alpha 反转 AND mask）
/// 与 commands/dsh.rs::ensure_big_hicon 的像素转换完全同源（Windows CreateIcon 固定格式），
/// 此处为集成测试独立复刻以验证真实窗口链路；若改格式两侧需同步。
fn create_icon_from_rgba(rgba: &[u8], width: u32, height: u32) -> windows::Win32::UI::WindowsAndMessaging::HICON {
    let pixel_count = (width * height) as usize;
    let mut bgra = Vec::with_capacity(rgba.len());
    let mut and_mask = Vec::with_capacity(pixel_count);
    for px in rgba.chunks_exact(4) {
        bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        and_mask.push(px[3].wrapping_sub(u8::MAX));
    }
    unsafe {
        CreateIcon(None, width as i32, height as i32, 1, 32, and_mask.as_ptr(), bgra.as_ptr())
            .expect("CreateIcon")
    }
}

/// 查询 HICON 实际像素尺寸（GetIconInfo → hbmColor → DIBSECTION/GetObject）
fn hicon_size(hicon: windows::Win32::UI::WindowsAndMessaging::HICON) -> (i32, i32) {
    use windows::Win32::Graphics::Gdi::{DIBSECTION, GetObjectW};
    let mut info = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: windows::Win32::Graphics::Gdi::HBITMAP(std::ptr::null_mut()),
        hbmColor: windows::Win32::Graphics::Gdi::HBITMAP(std::ptr::null_mut()),
    };
    let mut w = 0;
    let mut h = 0;
    unsafe {
        if GetIconInfo(hicon, &mut info).is_ok() && !info.hbmColor.0.is_null() {
            let mut dib = DIBSECTION::default();
            let n = GetObjectW(
                HGDIOBJ(info.hbmColor.0 as *mut _),
                std::mem::size_of::<DIBSECTION>() as i32,
                Some(&mut dib as *mut _ as *mut _),
            );
            if n > 0 {
                w = if dib.dsBmih.biWidth != 0 {
                    dib.dsBmih.biWidth
                } else {
                    dib.dsBm.bmWidth
                };
                h = if dib.dsBmih.biHeight != 0 {
                    dib.dsBmih.biHeight.abs()
                } else {
                    dib.dsBm.bmHeight
                };
            }
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0 as *mut _));
        }
        if !info.hbmMask.0.is_null() {
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0 as *mut _));
        }
    }
    (w, h)
}

/// 读窗口当前 ICON_BIG/SMALL 句柄的尺寸（WM_GETICON → hicon_size）
fn window_icon_size(hwnd: HWND, which: u32) -> (i32, i32) {
    unsafe {
        // WM_GETICON 返回句柄，需 SendMessageW 同步查询
        let res = SendMessageW(hwnd, WM_GETICON, Some(WPARAM(which as usize)), None);
        if res.0 == 0 {
            return (0, 0);
        }
        hicon_size(windows::Win32::UI::WindowsAndMessaging::HICON(res.0 as *mut core::ffi::c_void))
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[test]
fn test_window_big_icon_effective() {
    let class_name = "DshIconTestWnd";
    let wide_class: Vec<u16> = class_name.encode_utf16().chain(std::iter::once(0)).collect();
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        lpszClassName: PCWSTR(wide_class.as_ptr()),
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&wc) };
    assert!(atom != 0, "RegisterClassW 失败");

    let wide_title: Vec<u16> = "icon-test".encode_utf16().chain(std::iter::once(0)).collect();
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(wide_class.as_ptr()),
            PCWSTR(wide_title.as_ptr()),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            400,
            300,
            None,
            None,
            None,
            None,
        )
        .expect("CreateWindowExW")
    };

    // 生产同款 256px 源（ICON_BIG）
    let png = include_bytes!("../icons/128x128@2x.png");
    let img = tauri::image::Image::from_bytes(png).expect("PNG 解码");
    let hicon = create_icon_from_rgba(img.rgba(), img.width(), img.height());
    let (src_w, src_h) = hicon_size(hicon);
    println!("CreateIcon 实际尺寸: {src_w}x{src_h}");
    assert_eq!((src_w, src_h), (256, 256), "CreateIcon 应保留 256px");

    // 生产同款 512px 小图标源（ICON_SMALL，tao set_icon 内部同款转换）——
    // 验证 512 源经同款 RGBA→CreateIcon 转换后仍保留原生 512px（对齐 CHANGELOG 声明）
    let small_png = include_bytes!("../icons/icon.png");
    let small_img = tauri::image::Image::from_bytes(small_png).expect("512px PNG 解码");
    let small_hicon = create_icon_from_rgba(small_img.rgba(), small_img.width(), small_img.height());
    let (small_w, small_h) = hicon_size(small_hicon);
    println!("512px 源 CreateIcon 实际尺寸: {small_w}x{small_h}");
    assert_eq!((small_w, small_h), (512, 512), "512px 源应保留原生分辨率");
    unsafe {
        let _ = DestroyIcon(small_hicon);
    }

    // 应用 ICON_BIG + ICON_SMALL（与 apply_window_icon 相同）
    unsafe {
        let _ = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_BIG as usize)), Some(LPARAM(hicon.0 as isize)));
        let _ = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_SMALL as usize)), Some(LPARAM(hicon.0 as isize)));
    }

    // 读回验证
    let (big_w, big_h) = window_icon_size(hwnd, ICON_BIG);
    let (small_w, small_h) = window_icon_size(hwnd, ICON_SMALL);
    println!("窗口 ICON_BIG → {big_w}x{big_h}   ICON_SMALL → {small_w}x{small_h}");
    assert_eq!((big_w, big_h), (256, 256), "ICON_BIG 应保留 256px 原生（读回 {big_w}x{big_h}）");

    unsafe {
        let _ = DestroyIcon(hicon);
        let _ = DestroyWindow(hwnd);
    }
    println!("✅ 端到端验证通过：真实窗口 ICON_BIG = 256px 原生高清（任务栏将高质量缩放显示）");
}
