# 布局验证脚本（仅开发验证用）
# 验证：titlebar(窗口标题栏) / 右下日志贯穿 / 展开收起 / 拖拽 resize / 持久化 / 响应式断点
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

    # 0. 主窗口标题栏（无边框自绘）
    titlebar = page.locator(".titlebar")
    tb = titlebar.bounding_box()
    check("titlebar 存在且高度约 40", bool(tb) and abs((tb["height"] if tb else 0) - 40) < 4, f"h={tb['height'] if tb else 0:.0f}")
    has_drag = page.evaluate("document.querySelector('.titlebar').hasAttribute('data-tauri-drag-region')")
    check("titlebar 为拖拽区(drag-region)", has_drag)

    # 1. 三栏结构（右上角 titlebar 下）
    sb = page.locator(".sidebar-container").bounding_box()
    mb = page.locator(".app-main").bounding_box()
    rb = page.locator(".right-panel-container").bounding_box()
    check("三栏存在", bool(sb and mb and rb), f"sidebar.w={sb['width'] if sb else 0:.0f} main.w={mb['width'] if mb else 0:.0f} right.w={rb['width'] if rb else 0:.0f}")
    check("Sidebar 默认宽 260", abs((sb["width"] if sb else 0) - 260) < 2, f"width={sb['width'] if sb else 0:.0f}")
    check("Right Panel 默认宽 640", abs((rb["width"] if rb else 0) - 640) < 2, f"width={rb['width'] if rb else 0:.0f}")

    # 2. 右侧日志面板贯穿窗口顶（y=0，覆盖标题栏右侧区域）
    check("日志面板贯穿窗口顶(y=0)", bool(rb) and abs((rb["y"] if rb else 999) - 0) < 2, f"y={rb['y'] if rb else 0:.0f}")

    # 3. 标题栏按钮顺序：版本|最小化|最大化|关闭|日志收起（日志在关闭按钮右边=最后；设置已移到侧栏底部）
    tb_btns = page.locator(".titlebar button").count()
    check("标题栏按钮数=5(侧栏/最小/最大/关闭/日志)", tb_btns == 5, f"count={tb_btns}")
    log_btn = page.locator(".titlebar button").nth(4).inner_text()
    check("日志按钮在标题栏最右(关闭按钮右边)", "收起" in log_btn or "日志" in log_btn, f"text={log_btn!r}")
    # 侧栏底部设置按钮（独立区域）
    sidebar_set = page.locator(".sidebar-content button[title=\"打开设置\"]")
    check("侧栏底部存在设置按钮", sidebar_set.count() == 1)
    sb_footer_box = sidebar_set.bounding_box()
    sb_box = page.locator(".sidebar-container").bounding_box()
    check("设置按钮位于侧栏底部区域", bool(sb_footer_box and sb_box) and sb_footer_box["y"] > sb_box["y"] + sb_box["height"] * 0.7, f"y={sb_footer_box['y'] if sb_footer_box else 0:.0f}")

    # 4. Sidebar 展开/收起 → 收起后 Main 扩展 → 再展开恢复 260
    sidebar_toggle = page.locator(".titlebar button").first
    sidebar_toggle.click()
    page.wait_for_timeout(450)
    sb_closed = page.locator(".sidebar-container").bounding_box()
    mb_after_close = page.locator(".app-main").bounding_box()
    check("Sidebar 收起 -> 宽度 0", abs((sb_closed["width"] if sb_closed else 0)) < 2, f"width={sb_closed['width'] if sb_closed else 0:.0f}")
    check("收起后 Main 扩展", mb_after_close["width"] > mb["width"] + 100, f"{mb['width']:.0f}->{mb_after_close['width']:.0f}")
    sidebar_toggle.click()
    page.wait_for_timeout(450)
    sb_reopen = page.locator(".sidebar-container").bounding_box()
    check("再展开恢复 260", abs((sb_reopen["width"] if sb_reopen else 0) - 260) < 2, f"width={sb_reopen['width'] if sb_reopen else 0:.0f}")

    # 5. 日志面板收起/展开（标题栏日志按钮）
    right_toggle = page.locator(".titlebar button").nth(4)
    right_toggle.click()
    page.wait_for_timeout(450)
    rb_closed = page.locator(".right-panel-container").bounding_box()
    check("日志收起 -> 宽度 0", abs((rb_closed["width"] if rb_closed else 0)) < 2, f"width={rb_closed['width'] if rb_closed else 0:.0f}")
    btn_text_closed = right_toggle.inner_text()
    check("收起后按钮变[日志]", "日志" in btn_text_closed, f"text={btn_text_closed!r}")
    right_toggle.click()
    page.wait_for_timeout(450)
    rb_reopen = page.locator(".right-panel-container").bounding_box()
    check("日志再展开恢复 640", abs((rb_reopen["width"] if rb_reopen else 0) - 640) < 2, f"width={rb_reopen['width'] if rb_reopen else 0:.0f}")

    # 6. Sidebar 拖拽 260->320
    sb_handle = page.locator(".sidebar-resize-handle")
    sbh = sb_handle.bounding_box()
    page.mouse.move(sbh["x"] + sbh["width"] / 2, 450)
    page.mouse.down()
    page.mouse.move(sbh["x"] + sbh["width"] / 2 + 60, 450, steps=5)
    page.mouse.up()
    page.wait_for_timeout(350)
    sb_after = page.locator(".sidebar-container").bounding_box()
    check("Sidebar 拖拽 260->320", abs((sb_after["width"] if sb_after else 0) - 320) < 4, f"width={sb['width']:.0f}->{sb_after['width'] if sb_after else 0:.0f}")

    # 7. Right 拖拽 640->560
    r_handle = page.locator(".right-panel-resize-handle")
    rbh = r_handle.bounding_box()
    page.mouse.move(rbh["x"] + rbh["width"] / 2, 450)
    page.mouse.down()
    page.mouse.move(rbh["x"] + rbh["width"] / 2 + 80, 450, steps=5)
    page.mouse.up()
    page.wait_for_timeout(350)
    rb_after = page.locator(".right-panel-container").bounding_box()
    check("Right 拖拽 640->560", abs((rb_after["width"] if rb_after else 0) - 560) < 4, f"width={rb['width']:.0f}->{rb_after['width'] if rb_after else 0:.0f}")

    # 8. 持久化 + 重载恢复
    stored_side = page.evaluate("localStorage.getItem('dsh-launcher-sidebar-width')")
    stored_right = page.evaluate("localStorage.getItem('dsh-launcher-right-panel-width')")
    check("宽度已持久化", stored_side == "320" and stored_right == "560", f"sidebar={stored_side} right={stored_right}")
    page.reload(wait_until="networkidle")
    page.wait_for_timeout(900)
    sb_restored = page.locator(".sidebar-container").bounding_box()
    rb_restored = page.locator(".right-panel-container").bounding_box()
    check("重载后恢复宽度", abs((sb_restored["width"] if sb_restored else 0) - 320) < 2 and abs((rb_restored["width"] if rb_restored else 0) - 560) < 2, f"sidebar={sb_restored['width'] if sb_restored else 0:.0f} right={rb_restored['width'] if rb_restored else 0:.0f}")

    # 8a. 卸载按钮在重启右侧 + 版本管理面板自适应高度 + 左侧描边贯穿窗口顶
    status_btns = page.locator(".sidebar-content button")
    btn_texts = [status_btns.nth(i).inner_text() for i in range(status_btns.count())]
    # 按钮组顺序：…重启…卸载（重启索引 < 卸载索引）
    restart_idx = next((i for i, t in enumerate(btn_texts) if "重启" in t), -1)
    uninstall_idx = next((i for i, t in enumerate(btn_texts) if t == "卸载" or t.startswith("卸载")), -1)
    check("卸载按钮位于重启右侧", restart_idx >= 0 and uninstall_idx > restart_idx, f"restart={restart_idx} uninstall={uninstall_idx}")
    # 版本管理 Card 高度自适应窗口（main 高度 - 内边距 ≈ 860）
    vcard = page.locator(".app-main .group\\/card, .app-main [data-slot=card]").first
    vc = vcard.bounding_box()
    mb_full = page.locator(".app-main").bounding_box()
    check("版本管理卡片自适应窗口高度", bool(vc and mb_full) and vc["height"] > mb_full["height"] * 0.8, f"card.h={vc['height'] if vc else 0:.0f} main.h={mb_full['height'] if mb_full else 0:.0f}")
    # 左侧描边：贯穿整列的单条竖线（app-left::after，从窗口顶到底），位置=当前侧栏右缘
    line = page.evaluate(
        "() => {"
        "  const el = document.querySelector('.app-left');"
        "  const cs = getComputedStyle(el, '::after');"
        "  const r = el.getBoundingClientRect();"
        "  const sb = document.querySelector('.sidebar-container').getBoundingClientRect();"
        "  return { w: cs.width, top: r.top, bottom: r.bottom, x: parseFloat(cs.left), sbRight: sb.left + sb.width };"
        "}"
    )
    check(
        "左侧描边贯穿整列且对齐侧栏右缘",
        line["w"] == "1px" and line["top"] == 0 and line["bottom"] >= 850 and abs(line["x"] - line["sbRight"]) < 2,
        f"w={line['w']} top={line['top']:.0f} bottom={line['bottom']:.0f} x={line['x']:.0f} sbRight={line['sbRight']:.0f}",
    )

    # 8b. 双通道左右排列（桌面 1600 档 main≈698≥640 两列）+ 每通道最多 8 条
    npm_h = page.locator("text=npm 通道").bounding_box()
    gh_h = page.locator("text=GitHub 通道").bounding_box()
    check("双通道左右排列(标题同排)", bool(npm_h and gh_h) and abs((npm_h["y"] if npm_h else 999) - (gh_h["y"] if gh_h else 0)) < 4 and npm_h["x"] < gh_h["x"], f"npm y={npm_h['y'] if npm_h else 0:.0f} gh y={gh_h['y'] if gh_h else 0:.0f}")
    install_btns = page.get_by_role("button", name="安装", exact=True).count() + page.get_by_role("button", name="当前版本", exact=True).count()
    check("每通道最多 8 条(安装按钮总数≤16)", install_btns <= 16, f"count={install_btns}")

    # 9. 紧凑档 800px：Right 不参与 split（fixed overlay），titlebar 横贯
    page.set_viewport_size({"width": 800, "height": 900})
    page.wait_for_timeout(600)
    mb_c = page.locator(".app-main").bounding_box()
    rb_c = page.locator(".right-panel-container").bounding_box()
    check("紧凑档两栏", bool(mb_c) and mb_c["width"] < 800, f"main={mb_c['width'] if mb_c else 0:.0f}")
    check("紧凑档 Right 为 fixed overlay", bool(rb_c) and rb_c["x"] >= 0 and rb_c["width"] >= 300, f"x={rb_c['x'] if rb_c else 999:.0f} w={rb_c['width'] if rb_c else 0:.0f}")

    # 10. 移动端 600px：Main 占满，无溢出
    page.set_viewport_size({"width": 600, "height": 850})
    page.wait_for_timeout(600)
    mb_m = page.locator(".app-main").bounding_box()
    overflow = page.evaluate("document.querySelector('.app-main') ? document.querySelector('.app-main').scrollWidth <= document.querySelector('.app-main').clientWidth + 1 : true")
    check("移动端 Main 占满", bool(mb_m) and abs(mb_m["width"] - 600) < 4, f"main={mb_m['width'] if mb_m else 0:.0f}")
    check("内容无横向溢出", overflow)

    # 12. 无致命 JS 错误（忽略 Tauri API 缺失的 reject，各组件已 catch）
    fatal = [e for e in errors if "tauri" not in e and "invoke" not in e and "getCurrent" not in e]
    check("无致命 JS 错误", len(fatal) == 0, " | ".join(fatal) or "ok")

    browser.close()

failed = [r for r in results if not r[1]]
print(f"\n===== {len(results) - len(failed)}/{len(results)} 通过 =====")
exit(1 if failed else 0)