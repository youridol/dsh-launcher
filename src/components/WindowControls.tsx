// 自绘窗口控制按钮（无边框窗口的主窗口标题栏右侧）：最小化 / 最大化(还原) / 关闭
// 权限：capabilities/default.json 已补 allow-minimize / toggle-maximize / close / is-maximized
import { useEffect, useRef, useState } from "react";
import { getCurrentWindow, type Window } from "@tauri-apps/api/window";
import { Button } from "@/components/ui/button";
import { Copy, Minus, Square, X } from "lucide-react";

export default function WindowControls() {
  const [maximized, setMaximized] = useState(false);
  // 惰性获取窗口句柄：非 Tauri 环境（浏览器预览）getCurrentWindow() 会抛错，此时按钮不可用但不崩溃
  const winRef = useRef<Window | null>(null);

  // 挂载后获取窗口句柄 + 初始最大化状态 + 监听尺寸变化保持同步
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        const win = getCurrentWindow();
        winRef.current = win;
        setMaximized(await win.isMaximized());
        unlisten = await win.onResized(() => {
          // 尺寸变化后刷新状态（最大化/还原切换都会触发）
          win.isMaximized().then(setMaximized).catch(() => {});
        });
      } catch {
        // 非 Tauri 环境（浏览器预览）忽略，按钮保持禁用态不可用
      }
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  // 统一窗口操作：句柄未就绪时静默返回
  const act = (fn: (w: Window) => Promise<void>) => () => {
    const w = winRef.current;
    if (!w) return;
    fn(w).catch(() => {});
  };

  return (
    <div className="flex items-center">
      <Button
        variant="ghost"
        size="icon-sm"
        title="最小化"
        onClick={act((w) => w.minimize())}
      >
        <Minus className="size-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        title={maximized ? "还原" : "最大化"}
        onClick={act((w) => w.toggleMaximize())}
      >
        {maximized ? <Copy className="size-3" /> : <Square className="size-3" />}
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        title="关闭"
        onClick={act((w) => w.close())}
      >
        <X className="size-3.5" />
      </Button>
    </div>
  );
}