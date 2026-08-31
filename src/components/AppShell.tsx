// 三栏布局骨架（复刻 pi-agent-desktop 布局架构；组件/视觉零改动，仅重组容器）
// 结构：Left Sidebar | Main Content | Right Panel
// - 宽度由 CSS 变量控制（--sidebar-width / --right-panel-width），与 open/close 状态分离
// - 拖拽调整宽度（useResizablePanel：pointer → clamp → CSS 变量 → 持久化）
// - 响应式：≥960px 三栏；641~959px 两栏（Right Panel 改 overlay 不参与 split）；
//   ≤640px 移动端（Sidebar 改 drawer，Main 占满，Right Panel 全屏 overlay）
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import StatusCard from "@/components/StatusCard";
import ToolchainPanel from "@/components/ToolchainPanel";
import VersionPanel from "@/components/VersionPanel";
import LogPanel from "@/components/LogPanel";
import { useResizablePanel } from "@/hooks/useResizablePanel";
import { useIsMobile } from "@/hooks/useIsMobile";
import {
  getDefaultRightPanelWidth,
  getRightPanelMaxWidth,
  getSidebarMaxWidth,
  RIGHT_PANEL_MAX_WIDTH,
  RIGHT_PANEL_MIN_WIDTH,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
} from "@/lib/panel-layout";
import {
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Settings as SettingsIcon,
} from "lucide-react";

interface AppShellProps {
  /** 应用版本号（来自 tauri，如 "v0.3.9"） */
  version: string;
  /** 打开设置弹出子窗口（App.tsx 管理 Dialog 状态） */
  onOpenSettings: () => void;
}

export default function AppShell({ version, onOpenSettings }: AppShellProps) {
  // 面板展开/收起状态（与宽度值分离：宽度持久化于 localStorage）
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [rightOpen, setRightOpen] = useState(true);
  const isMobile = useIsMobile();

  // 当前宽度值（ref 供拖拽/响应式计算实时读取；拖拽结束持久化）
  const sidebarWidthRef = useRef(SIDEBAR_DEFAULT_WIDTH);
  const rightPanelWidthRef = useRef(
    typeof window === "undefined" ? 560 : getDefaultRightPanelWidth(window.innerWidth),
  );

  // 右侧 Panel 响应式默认宽度（视口 42%，360~640）
  const getResponsiveRightPanelWidth = useCallback(
    () =>
      typeof window === "undefined" ? 560 : getDefaultRightPanelWidth(window.innerWidth),
    [],
  );

  // 左侧 Sidebar 响应式最大宽度：视口 − 主内容最小宽 − 展开的右栏宽
  const getResponsiveSidebarMaxWidth = useCallback(
    () =>
      typeof window === "undefined"
        ? SIDEBAR_MAX_WIDTH
        : getSidebarMaxWidth({
            viewportWidth: window.innerWidth,
            rightPanelOpen: rightOpen,
            rightPanelWidth: rightPanelWidthRef.current,
          }),
    [rightOpen],
  );

  // 右侧 Panel 响应式最大宽度：视口 − 主内容最小宽 − 展开的侧栏宽
  const getResponsiveRightPanelMaxWidth = useCallback(
    () =>
      typeof window === "undefined"
        ? RIGHT_PANEL_MAX_WIDTH
        : getRightPanelMaxWidth({
            viewportWidth: window.innerWidth,
            sidebarOpen,
            sidebarWidth: sidebarWidthRef.current,
          }),
    [sidebarOpen],
  );

  const sidebarResizer = useResizablePanel({
    ariaLabel: "调整左侧栏宽度",
    cssVariable: "--sidebar-width",
    defaultWidth: SIDEBAR_DEFAULT_WIDTH,
    getMaxWidth: getResponsiveSidebarMaxWidth,
    growthDirection: "right",
    maxWidth: SIDEBAR_MAX_WIDTH,
    minWidth: SIDEBAR_MIN_WIDTH,
    storageKey: "dsh-launcher-sidebar-width",
    widthRef: sidebarWidthRef,
  });

  const rightResizer = useResizablePanel({
    ariaLabel: "调整日志栏宽度",
    cssVariable: "--right-panel-width",
    // 首帧即用响应式宽度（与 ref 一致），避免挂载后从 fallback 宽度动画到默认值
    defaultWidth:
      typeof window === "undefined" ? 560 : getDefaultRightPanelWidth(window.innerWidth),
    getDefaultWidth: getResponsiveRightPanelWidth,
    getMaxWidth: getResponsiveRightPanelMaxWidth,
    growthDirection: "left",
    maxWidth: RIGHT_PANEL_MAX_WIDTH,
    minWidth: RIGHT_PANEL_MIN_WIDTH,
    storageKey: "dsh-launcher-right-panel-width",
    widthRef: rightPanelWidthRef,
  });

  // 进入移动端自动收起侧栏（drawer 默认收起）
  useEffect(() => {
    if (isMobile) setSidebarOpen(false);
  }, [isMobile]);

  // 视口缩到紧凑档（641~959px）自动收起右栏（该档位不参与 split 布局）
  useEffect(() => {
    if (typeof window === "undefined") return;
    const mql = window.matchMedia("(min-width: 641px) and (max-width: 959px)");
    const onChange = () => {
      if (mql.matches) setRightOpen(false);
    };
    onChange();
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, []);

  return (
    <div className="app-shell bg-background text-foreground">
      {/* 移动端侧栏遮罩（点击关闭 drawer） */}
      <div
        className={`sidebar-overlay-backdrop${isMobile && sidebarOpen ? " is-open" : ""}`}
        onClick={() => setSidebarOpen(false)}
      />

      {/* 左侧 Sidebar：dsh 运行状态 + 工具链 */}
      <aside
        ref={sidebarResizer.panelRef}
        className={`sidebar-container${sidebarOpen ? " sidebar-open" : " sidebar-closed"}${sidebarResizer.isResizing ? " sidebar-resizing" : ""}`}
        style={{ "--sidebar-width": `${sidebarResizer.width}px` } as CSSProperties}
        aria-label="左侧栏"
      >
        <div className="sidebar-content">
          <StatusCard />
          <Separator className="my-4" />
          <ToolchainPanel />
        </div>
      </aside>

      {/* 侧栏拖拽 handle（仅桌面三栏；紧凑/移动档 CSS 隐藏） */}
      {sidebarOpen && (
        <div
          {...sidebarResizer.separatorProps}
          className={`panel-resize-handle sidebar-resize-handle${sidebarResizer.isResizing ? " is-resizing" : ""}`}
        />
      )}

      {/* 中央 Main：标题栏 + 版本管理 */}
      <main className="app-main">
        <header className="app-header">
          <div className="flex min-w-0 items-center gap-2">
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={() => setSidebarOpen(!sidebarOpen)}
              title={sidebarOpen ? "收起左侧栏" : "展开左侧栏"}
            >
              {sidebarOpen ? <PanelLeftClose /> : <PanelLeftOpen />}
            </Button>
            <div className="min-w-0">
              <h1 className="truncate text-lg font-semibold">dsh-launcher</h1>
              <p className="truncate text-xs text-muted-foreground">
                deepseek-harness 启动器
              </p>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <span className="text-xs text-muted-foreground">{version}</span>
            <Button
              variant="outline"
              size="sm"
              onClick={onOpenSettings}
              title="打开设置"
            >
              <SettingsIcon className="size-3.5" /> 设置
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={() => setRightOpen(!rightOpen)}
              title={rightOpen ? "收起日志" : "展开日志"}
            >
              {rightOpen ? <PanelRightClose /> : <PanelRightOpen />}
            </Button>
          </div>
        </header>
        <div className="app-main-body">
          <VersionPanel />
        </div>
      </main>

      {/* 右栏拖拽 handle（仅桌面三栏；紧凑/移动档 CSS 隐藏） */}
      {rightOpen && (
        <div
          {...rightResizer.separatorProps}
          className={`panel-resize-handle right-panel-resize-handle${rightResizer.isResizing ? " is-resizing" : ""}`}
        />
      )}

      {/* 右侧 Panel：日志（≥960px 参与 split；紧凑档为 fixed overlay；移动端全屏） */}
      <aside
        ref={rightResizer.panelRef}
        className={`right-panel-container${rightOpen ? " right-panel-open" : " right-panel-closed"}${rightResizer.isResizing ? " right-panel-resizing" : ""}`}
        style={{ "--right-panel-width": `${rightResizer.width}px` } as CSSProperties}
        aria-label="日志面板"
      >
        <LogPanel className="h-full" onClose={() => setRightOpen(false)} />
      </aside>

      {/* 紧凑/移动档右栏遮罩（点击关闭 overlay） */}
      <div
        className={`right-panel-overlay-backdrop${rightOpen ? " is-open" : ""}`}
        onClick={() => setRightOpen(false)}
      />
    </div>
  );
}