# 布局验证脚本（仅开发验证用，不属于发布产物）
# 验证：三栏结构 / 展开收起 / 拖拽 resize / 持久化恢复 / 响应式断点
from playwright.sync_api import sync_playwright

BASE = "http://localhost:4173"
results = []

def check(name, ok, detail=""):
    results.append((name, ok))
    print(("PASS" if ok else "FAIL") + "  " + name + (("  ->  " + detail) if detail else ""))

with sync_playwright() as p:
    browser = p.chromium.launch()
    page = browser.new_page(viewport={"width": 1600, "height": 900})
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))

    page.goto(BASE, wait_until="networkidle")
    page.wait_for_timeout(900)

    # 1. 三栏结构
    sb = page.locator(".sidebar-container").bounding_box()
    mb = page.locator(".app-main").bounding_box()
    rb = page.locator(".right-panel-container").bounding_box()
    check("三栏存在", bool(sb and mb and rb), f"sidebar.w={sb['width']:.0f} main.w={mb['width']:.0f} right.w={rb['width']:.0f}")
    check("Sidebar 默认宽 260", abs((sb["width"] if sb else 0) - 260) < 2, f"width={sb['width']:.0f}")
    check("Right Panel 默认宽 640(0.42vw clamp)", abs((rb["width"] if rb else 0) - 640) < 2, f"width={rb['width']:.0f}")
    check("Main 占剩余空间", abs((sb["width"] + mb["width"] + rb["width"]) - 1600) < 4, f"sum={sb['width']+mb['width']+rb['width']:.0f}")

    # 2. Sidebar 展开/收起 → 收起后 Main 扩展 → 展开恢复 260
    sidebar_toggle = page.locator(".app-header button").first
    sidebar_toggle.click()
    page.wait_for_timeout(450)
    sb_closed = page.locator(".sidebar-container").bounding_box()
    mb_after_close = page.locator(".app-main").bounding_box()
    check("Sidebar 收起 -> 宽度 0", abs((sb_closed["width"] if sb_closed else 0)) < 2, f"width={sb_closed['width']:.0f}")
    check("收起后 Main 扩展", mb_after_close["width"] > mb["width"] + 100, f"{mb['width']:.0f}->{mb_after_close['width']:.0f}")
    sidebar_toggle.click()
    page.wait_for_timeout(450)
    sb_reopen = page.locator(".sidebar-container").bounding_box()
    check("再展开恢复 260", abs((sb_reopen["width"] if sb_reopen else 0) - 260) < 2, f"width={sb_reopen['width']:.0f}")

    # 3. Right Panel 展开/收起
    right_toggle = page.locator(".app-header button").nth(2)
    right_toggle.click()
    page.wait_for_timeout(450)
    rb_closed = page.locator(".right-panel-container").bounding_box()
    check("Right Panel 收起 -> 宽度 0", abs((rb_closed["width"] if rb_closed else 0)) < 2, f"width={rb_closed['width']:.0f}")
    right_toggle.click()
    page.wait_for_timeout(450)
    rb_reopen = page.locator(".right-panel-container").bounding_box()
    check("Right Panel 再展开恢复", abs((rb_reopen["width"] if rb_reopen else 0) - 640) < 2, f"width={rb_reopen['width']:.0f}")

    # 4. Sidebar 拖拽调宽 260 -> 320（handle 向右拖 60px）
    sb_handle = page.locator(".sidebar-resize-handle")
    sbh = sb_handle.bounding_box()
    page.mouse.move(sbh["x"] + sbh["width"] / 2, 450)
    page.mouse.down()
    page.mouse.move(sbh["x"] + sbh["width"] / 2 + 60, 450, steps=5)
    page.mouse.up()
    page.wait_for_timeout(350)
    sb_after = page.locator(".sidebar-container").bounding_box()
    check("Sidebar 拖拽 260->320", abs((sb_after["width"] if sb_after else 0) - 320) < 4, f"width={sb['width']:.0f}->{sb_after['width']:.0f}")

    # 5. Right Panel 拖拽 640 -> 560（向右拖 80px 变窄）
    r_handle = page.locator(".right-panel-resize-handle")
    rbh = r_handle.bounding_box()
    page.mouse.move(rbh["x"] + rbh["width"] / 2, 450)
    page.mouse.down()
    page.mouse.move(rbh["x"] + rbh["width"] / 2 + 80, 450, steps=5)
    page.mouse.up()
    page.wait_for_timeout(350)
    rb_after = page.locator(".right-panel-container").bounding_box()
    check("Right Panel 拖拽 640->560", abs((rb_after["width"] if rb_after else 0) - 560) < 4, f"width={rb['width']:.0f}->{rb_after['width']:.0f}")

    # 6. 持久化 + 重载恢复
    stored_side = page.evaluate("localStorage.getItem('dsh-launcher-sidebar-width')")
    stored_right = page.evaluate("localStorage.getItem('dsh-launcher-right-panel-width')")
    check("宽度已持久化", stored_side == "320" and stored_right == "560", f"sidebar={stored_side} right={stored_right}")
    page.reload(wait_until="networkidle")
    page.wait_for_timeout(900)
    sb_restored = page.locator(".sidebar-container").bounding_box()
    rb_restored = page.locator(".right-panel-container").bounding_box()
    check("重载后恢复宽度", abs((sb_restored["width"] if sb_restored else 0) - 320) < 2 and abs((rb_restored["width"] if rb_restored else 0) - 560) < 2, f"sidebar={sb_restored['width']:.0f} right={rb_restored['width']:.0f}")

    # 7. 紧凑档 800px：Right Panel 不参与 split（fixed overlay）
    page.set_viewport_size({"width": 800, "height": 900})
    page.wait_for_timeout(600)
    sb_c = page.locator(".sidebar-container").bounding_box()
    mb_c = page.locator(".app-main").bounding_box()
    rb_c = page.locator(".right-panel-container").bounding_box()
    check("紧凑档两栏(Sidebar|Main)", sb_c and abs((sb_c["width"] + mb_c["width"]) - 800) < 4, f"sidebar={sb_c['width']:.0f} main={mb_c['width']:.0f}")
    check("紧凑档 Right Panel 为固定 overlay", rb_c and rb_c["x"] >= 0 and rb_c["width"] >= 300, f"x={rb_c['x']:.0f} w={rb_c['width']:.0f}")

    # 8. 移动端 600px：Sidebar 变 drawer 隐藏，Main 占满；窗口缩放不溢出
    page.set_viewport_size({"width": 600, "height": 850})
    page.wait_for_timeout(600)
    mb_m = page.locator(".app-main").bounding_box()
    check("移动端 Main 占满", mb_m and abs(mb_m["width"] - 600) < 4, f"main={mb_m['width']:.0f}")
    # main-body 内容不横向溢出
    overflow = page.evaluate("document.querySelector('.app-main-body') ? document.querySelector('.app-main-body').scrollWidth <= document.querySelector('.app-main-body').clientWidth + 1 : true")
    check("内容无横向溢出", overflow)

    # 9. 无致命 JS 错误（忽略 Tauri API 缺失的 reject，各组件已 catch）
    fatal = [e for e in errors if "tauri" not in e and "invoke" not in e and "getCurrent" not in e]
    check("无致命 JS 错误", len(fatal) == 0, " | ".join(fatal) or "ok")

    browser.close()

failed = [r for r in results if not r[1]]
print(f"\n===== {len(results) - len(failed)}/{len(results)} 通过 =====")
exit(1 if failed else 0)