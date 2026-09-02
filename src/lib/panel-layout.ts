// 三栏布局（复刻 pi-agent-desktop 布局架构）的宽度常量与计算
// 仅布局逻辑；视觉样式与业务无关
// 状态分离：宽度值（useResizablePanel 持久化）与 open/close 状态独立

/** 移动端断点（≤640px：Sidebar 变 overlay/drawer，Main 占满） */
export const MOBILE_MAX_WIDTH = 640;
/** 紧凑桌面断点（<960px：仅 Sidebar | Main 两栏，Right Panel 不参与 split） */
export const SPLIT_PANEL_MIN_WIDTH = 960;

/** 左侧 Sidebar：默认 / 最小 / 最大宽度（默认 340px，用户拖拽可在 200~480 调整） */
export const SIDEBAR_DEFAULT_WIDTH = 340;
export const SIDEBAR_MIN_WIDTH = 200;
export const SIDEBAR_MAX_WIDTH = 480;

/** 右侧 Panel：默认固定 340px / 最小 / 最大宽度 */
export const RIGHT_PANEL_DEFAULT_WIDTH = 340;
export const RIGHT_PANEL_MIN_WIDTH = 280;
export const RIGHT_PANEL_MAX_WIDTH = 1200;

/** 主内容区允许的最小宽度（桌面 420 / 紧凑 320），防止面板挤压溢出 */
const COMPACT_CHAT_MIN_WIDTH = 320;
const DESKTOP_CHAT_MIN_WIDTH = 420;

/** 宽度 clamp：处理 NaN/Infinity，并保证 min ≤ max */
export function clampPanelWidth(width: number, minWidth: number, maxWidth: number): number {
  const finiteWidth = Number.isFinite(width) ? width : minWidth;
  const effectiveMax = Math.max(minWidth, maxWidth);
  return Math.round(Math.max(minWidth, Math.min(effectiveMax, finiteWidth)));
}

/** 右侧 Panel 默认宽度：固定 340px（不再按视口响应式） */
export function getDefaultRightPanelWidth(_viewportWidth: number): number {
  return RIGHT_PANEL_DEFAULT_WIDTH;
}

/** 左侧 Sidebar 的最大可用宽度：视口减去主内容最小宽度与展开的右栏宽度 */
export function getSidebarMaxWidth(options: {
  viewportWidth: number;
  rightPanelOpen: boolean;
  rightPanelWidth: number;
}): number {
  const { viewportWidth, rightPanelOpen, rightPanelWidth } = options;
  if (viewportWidth <= MOBILE_MAX_WIDTH) return SIDEBAR_MAX_WIDTH;

  const compact = viewportWidth < SPLIT_PANEL_MIN_WIDTH;
  const chatWidth = compact ? COMPACT_CHAT_MIN_WIDTH : DESKTOP_CHAT_MIN_WIDTH;
  const visibleRightPanelWidth = !compact && rightPanelOpen ? rightPanelWidth : 0;
  return Math.min(SIDEBAR_MAX_WIDTH, viewportWidth - chatWidth - visibleRightPanelWidth);
}

/** 右侧 Panel 的最大可用宽度：视口减去主内容最小宽度与展开的侧栏宽度 */
export function getRightPanelMaxWidth(options: {
  viewportWidth: number;
  sidebarOpen: boolean;
  sidebarWidth: number;
}): number {
  const { viewportWidth, sidebarOpen, sidebarWidth } = options;
  if (viewportWidth < SPLIT_PANEL_MIN_WIDTH) return RIGHT_PANEL_MAX_WIDTH;

  const visibleSidebarWidth = sidebarOpen ? sidebarWidth : 0;
  return Math.min(
    RIGHT_PANEL_MAX_WIDTH,
    viewportWidth - DESKTOP_CHAT_MIN_WIDTH - visibleSidebarWidth,
  );
}