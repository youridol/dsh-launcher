// 三栏布局骨架（复刻 pi-agent-desktop 布局架构；组件/视觉零改动，仅重组容器）
// 结构：主窗口标题栏（无边框自绘，与侧边栏融合）+ [左侧栏 | 主区] | 右侧日志面板（贯穿窗口顶）
// - 宽度由 CSS 变量控制（--sidebar-width / --right-panel-width），与 open/close 状态分离
// - 拖拽调整宽度（useResizablePanel：pointer → clamp → CSS 变量 → 持久化）
// - 响应式：≥960px 三栏；641~959px 两栏（Right Panel 改 fixed overlay 不参与 split）；
//   ≤640px 移动端（Sidebar 变 drawer，Main 占满，Right Panel 全屏 overlay）
// 无边框主窗口：标题栏 data-tauri-drag-region 拖动窗口，双击空白处最大化/还原；
// 右上角自绘窗口控制（最小化/最大化/关闭）+ 日志收起/展开按钮（关闭按钮右边）
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import StatusCard from "@/components/StatusCard";
import VersionPanel from "@/components/VersionPanel";
import LogPanel from "@/components/LogPanel";
import WindowControls from "@/components/WindowControls";
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
  PanelRightOpen,
  Settings as SettingsIcon,
  X,
} from "lucide-react";

interface AppShellProps {
  /** 应用版本号（来自 tauri，如 "v0.3.10"） */
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
    typeof window === "undefined" ? 340 : getDefaultRightPanelWidth(window.innerWidth),
  );

  // 右侧 Panel 响应式默认宽度（视口 42%，360~640）
  const getResponsiveRightPanelWidth = useCallback(
    () =>
      typeof window === "undefined" ? 340 : getDefaultRightPanelWidth(window.innerWidth),
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
    storageKey: "dsh-launcher-sidebar-width-v2",
    widthRef: sidebarWidthRef,
  });

  const rightResizer = useResizablePanel({
    ariaLabel: "调整日志栏宽度",
    cssVariable: "--right-panel-width",
    // 首帧即用响应式宽度（与 ref 一致），避免挂载后从 fallback 宽度动画到默认值
    defaultWidth:
      typeof window === "undefined" ? 340 : getDefaultRightPanelWidth(window.innerWidth),
    getDefaultWidth: getResponsiveRightPanelWidth,
    getMaxWidth: getResponsiveRightPanelMaxWidth,
    growthDirection: "left",
    maxWidth: RIGHT_PANEL_MAX_WIDTH,
    minWidth: RIGHT_PANEL_MIN_WIDTH,
    storageKey: "dsh-launcher-right-panel-width-v2",
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

  // 双击标题栏空白处：最大化/还原（按钮区域不触发）
  const handleTitlebarDoubleClick = useCallback((e: ReactMouseEvent) => {
    if ((e.target as HTMLElement).closest("button")) return;
    try {
      void getCurrentWindow().toggleMaximize();
    } catch {
      // 非 Tauri 环境忽略
    }
  }, []);

  return (
    <div className="app-shell bg-background text-foreground">
      {/* 移动端侧栏遮罩（点击关闭 drawer） */}
      <div
        className={`sidebar-overlay-backdrop${isMobile && sidebarOpen ? " is-open" : ""}`}
        onClick={() => setSidebarOpen(false)}
      />

      {/* 左侧列：主窗口标题栏 + 内容行
          ref 用于实时写入 --sidebar-width（驱动侧栏宽度/竖线/标题栏让位），
          data-sidebar-open 控制收起态 */}
      <div className="app-left" ref={sidebarResizer.panelRef} data-sidebar-open={sidebarOpen}>
        {/* 左侧 Sidebar：absolute 覆盖全高（贯穿到窗口顶，与右侧日志面板视觉对称）
            宽度 = var(--sidebar-width)，随拖拽实时（变量继承自 .app-left inline） */}
        <aside
          className={`sidebar-container${sidebarOpen ? " sidebar-open" : " sidebar-closed"}${sidebarResizer.isResizing ? " sidebar-resizing" : ""}`}
          aria-label="左侧栏"
        >
          <div className="sidebar-content">
            {/* 侧栏头部（贯穿到窗口顶，与右侧日志面板头部对称）：标题（收起按钮已移至主内容左上方） */}
            <header className="sidebar-header">
              <div className="min-w-0">
                <h1 className="truncate text-sm font-semibold leading-tight">dsh-launcher</h1>
                <p className="truncate text-[11px] text-muted-foreground leading-tight">
                  deepseek-harness 启动器
                </p>
              </div>
            </header>
            <div className="sidebar-body">
              <StatusCard />
              {/* 底部：设置入口（分割线分隔，独立区域） */}
              <div className="mt-auto flex flex-col pt-4">
                <Separator className="mb-3" />
                <Button
                  variant="outline"
                  size="sm"
                  onClick={onOpenSettings}
                  title="打开设置"
                  className="w-full"
                >
                  <SettingsIcon className="size-3.5" /> 设置
                </Button>
              </div>
            </div>
          </div>
        </aside>

        {/* 侧栏拖拽 handle：贯穿全高（从窗口顶到底，与右侧 handle 对称），
            absolute 于 sidebar 右缘；紧凑/移动档 CSS 隐藏 */}
        {sidebarOpen && (
          <div
            {...sidebarResizer.separatorProps}
            className={`panel-resize-handle sidebar-resize-handle${sidebarResizer.isResizing ? " is-resizing" : ""}`}
          />
        )}

        {/* 主窗口标题栏（无边框自绘）：位于侧栏右侧；左侧放侧栏收起/展开按钮（主内容左上方），
            右侧整组（版本号 + 窗口控制 + 日志收起）贴近日志侧边栏；空白区域拖拽移动窗口 */}
        <header
          className="titlebar"
          data-tauri-drag-region
          onDoubleClick={handleTitlebarDoubleClick}
        >
          {/* 侧栏收起/展开按钮：主内容左上方（侧栏展开显示收起，收起显示展开） */}
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={() => setSidebarOpen(!sidebarOpen)}
            title={sidebarOpen ? "收起左侧栏" : "展开左侧栏"}
            className="shrink-0"
          >
            {sidebarOpen ? <PanelLeftClose /> : <PanelLeftOpen />}
          </Button>
          {/* 右侧控件组：版本号 + 最小化/最大化/关闭 + 日志收起（贴右缘，靠近右侧日志边栏） */}
          <div className="flex shrink-0 items-center gap-1.5">
            <span className="text-xs text-muted-foreground">{version}</span>
            {/* 自绘窗口控制按钮（无边框窗口必需） */}
            <WindowControls />
            {/* 日志收起/展开按钮：主窗口标题栏最右（仅图标，无文字） */}
            <Button
              variant="outline"
              size="icon-sm"
              onClick={() => setRightOpen(!rightOpen)}
              title={rightOpen ? "收起日志" : "展开日志"}
            >
              {rightOpen ? <X className="size-3.5" /> : <PanelRightOpen className="size-3.5" />}
            </Button>
          </div>
        </header>

        {/* 内容行：主区（版本管理），自动占满剩余空间 */}
        <div className="app-content">
          <main className="app-main">
            <VersionPanel />
          </main>
        </div>
      </div>

      {/* 右栏拖拽 handle（仅桌面三栏；紧凑/移动档 CSS 隐藏） */}
      {rightOpen && (
        <div
          {...rightResizer.separatorProps}
          className={`panel-resize-handle right-panel-resize-handle${rightResizer.isResizing ? " is-resizing" : ""}`}
        />
      )}

      {/* 右侧日志面板：从窗口顶贯穿到底（覆盖标题栏右侧区域；子窗口外观保留） */}
      <aside
        ref={rightResizer.panelRef}
        className={`right-panel-container${rightOpen ? " right-panel-open" : " right-panel-closed"}${rightResizer.isResizing ? " right-panel-resizing" : ""}`}
        style={{ "--right-panel-width": `${rightResizer.width}px` } as CSSProperties}
        aria-label="日志面板"
      >
        {/* onClose 移除：日志收起按钮已移至主窗口标题栏（关闭按钮右边） */}
        <LogPanel className="h-full" />
      </aside>

      {/* 紧凑/移动档右栏遮罩（点击关闭 overlay） */}
      <div
        className={`right-panel-overlay-backdrop${rightOpen ? " is-open" : ""}`}
        onClick={() => setRightOpen(false)}
      />
    </div>
  );
}