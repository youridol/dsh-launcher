// 三栏布局（复刻 pi-agent-desktop 布局架构）的宽度常量与计算
// 仅布局逻辑；视觉样式与业务无关
// 状态分离：宽度值（useResizablePanel 持久化）与 open/close 状态独立

/** 移动端断点（≤640px：Sidebar 变 overlay/drawer，Main 占满）。
 *
 * G7（审计 §2.1）：此前 `useIsMobile` 另行硬编码 `max-width: 640px`，与这里的常量
 * 形成双源（改一处忘另一处即漂移）。现由 `useIsMobile` 引用本常量，单一来源在 TS 侧；
 * `index.css:431` 的 `@media (max-width: 640px)` 是 CSS 侧无法共享的副本，
 * 修改时必须同步（两处都已标注）。
 */
export const MOBILE_MAX_WIDTH = 640;

/** 紧凑桌面断点（<960px：仅 Sidebar | Main 两栏，Right Panel 不参与 split）。
 *
 * G7（审计 §2.1）：仅在**本模块内部**（`getSidebarMaxWidth` / `getRightPanelMaxWidth`）
 * 使用，故不再 `export`（收紧公共面）。
 */
const SPLIT_PANEL_MIN_WIDTH = 960;

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

/**
 * 宽度 clamp：处理 NaN/Infinity，并保证 min ≤ max。
 *
 * 注（ADR-0009 D20，2026-09-12 复核后**保留原语义**）：曾怀疑
 * `effectiveMax = Math.max(minWidth, maxWidth)` 在「可用上限 < 设计下限」时会放弃上限
 * 而导致布局溢出，并试改为「上限优先」。但对**可达状态空间**（视口 641–1920 × 两栏
 * open/closed × 宽度取默认/最小/最大，含不动点迭代与单次遍历两种顺序，共 138,240 组）
 * 的穷举模拟显示：两侧 max 计算**互相约束**、系统总能收敛到不溢出的解，**未发现可达溢出**。
 * 而「上限优先」会把侧栏压到 120（低于 SIDEBAR_MIN_WIDTH=200）、违反自身声明的最小宽，
 * 属未经证实的行为变更，故回退。详见 ADR-0009 D20 的实测复核。
 */
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