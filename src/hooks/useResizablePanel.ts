// 面板拖拽调整宽度 hook（复刻 pi-agent-desktop 布局架构的 Resize 机制）
// 流程：Pointer 拖拽 → 计算新宽度 → Min/Max Clamp → 更新 CSS 变量 → 布局实时变化
//       → 拖拽结束 → 持久化宽度（localStorage）
// 宽度状态与 open/close 状态分离：本 hook 只管理宽度值
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MutableRefObject,
  type PointerEvent,
} from "react";
import { clampPanelWidth } from "@/lib/panel-layout";

/** 拖拽进行中的状态快照 */
interface DragState {
  pointerId: number;
  startX: number;
  startWidth: number;
  target: HTMLDivElement;
  previousCursor: string;
  previousUserSelect: string;
}

interface UseResizablePanelOptions {
  ariaLabel: string;
  /** 控制的 CSS 变量（如 --sidebar-width / --right-panel-width） */
  cssVariable: `--${string}`;
  defaultWidth: number;
  /** 响应式默认宽度（如右栏按视口 42% 计算） */
  getDefaultWidth?: () => number;
  /** 响应式最大宽度（随视口/另一栏状态变化） */
  getMaxWidth: () => number;
  /** 拖拽增长方向：右栏为 left（向左变宽），左栏为 right（向右变宽） */
  growthDirection: "left" | "right";
  maxWidth: number;
  minWidth: number;
  storageKey: string;
  widthRef: MutableRefObject<number>;
}

interface CommitOptions {
  forcePersist?: boolean;
  persist?: boolean;
}

/** 读取持久化宽度（读取失败/非法返回 null） */
function readStoredWidth(storageKey: string): number | null {
  try {
    const stored = window.localStorage.getItem(storageKey);
    if (stored === null) return null;
    const parsed = Number.parseInt(stored, 10);
    return Number.isFinite(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

/** 写入持久化宽度（存储不可用时静默失败，拖拽仍可用） */
function writeStoredWidth(storageKey: string, width: number): void {
  try {
    window.localStorage.setItem(storageKey, String(width));
  } catch {
    // 存储不可用时拖拽功能不受影响
  }
}

export function useResizablePanel(options: UseResizablePanelOptions) {
  const {
    ariaLabel,
    cssVariable,
    defaultWidth,
    getDefaultWidth,
    getMaxWidth,
    growthDirection,
    maxWidth,
    minWidth,
    storageKey,
    widthRef,
  } = options;
  const panelRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<DragState | null>(null);
  const restoredRef = useRef(false);
  const [width, setWidth] = useState(defaultWidth);
  const [isResizing, setIsResizing] = useState(false);

  /** 当前有效最大宽度：min(固定上限, 响应式可用宽度)，且不低于 minWidth */
  const effectiveMaxWidth = useCallback(
    () => Math.min(maxWidth, Math.max(minWidth, getMaxWidth())),
    [getMaxWidth, maxWidth, minWidth],
  );

  /** 候选宽度 clamp 到 [minWidth, effectiveMaxWidth] */
  const clampWidth = useCallback(
    (candidate: number) => clampPanelWidth(candidate, minWidth, effectiveMaxWidth()),
    [effectiveMaxWidth, minWidth],
  );

  /** 实时应用宽度：写 ref + CSS 变量（布局即随变量变化） */
  const applyLiveWidth = useCallback(
    (nextWidth: number) => {
      widthRef.current = nextWidth;
      panelRef.current?.style.setProperty(cssVariable, `${nextWidth}px`);
    },
    [cssVariable, widthRef],
  );

  /** 提交宽度：clamp 后应用，必要时持久化 */
  const commitWidth = useCallback(
    (candidate: number, commitOptions: CommitOptions = {}) => {
      const { forcePersist = false, persist = true } = commitOptions;
      const nextWidth = clampWidth(candidate);
      const changed = nextWidth !== widthRef.current;
      applyLiveWidth(nextWidth);
      setWidth(nextWidth);
      if (persist && (changed || forcePersist)) writeStoredWidth(storageKey, nextWidth);
      return nextWidth;
    },
    [applyLiveWidth, clampWidth, storageKey, widthRef],
  );

  /** 恢复拖拽前的 body 光标/选中状态 */
  const restoreBodyState = useCallback((drag: DragState) => {
    document.body.style.cursor = drag.previousCursor;
    document.body.style.userSelect = drag.previousUserSelect;
  }, []);

  /** 结束拖拽：释放捕获、恢复状态、强制持久化最终宽度 */
  const finishResize = useCallback(
    (pointerId: number) => {
      const drag = dragRef.current;
      if (!drag || drag.pointerId !== pointerId) return;
      dragRef.current = null;
      restoreBodyState(drag);
      setIsResizing(false);
      commitWidth(widthRef.current, { forcePersist: true });
      try {
        if (drag.target.hasPointerCapture(pointerId)) {
          drag.target.releasePointerCapture(pointerId);
        }
      } catch {
        // 指针已取消时浏览器可能已释放捕获
      }
    },
    [commitWidth, restoreBodyState, widthRef],
  );

  const onPointerDown = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      if (event.pointerType === "mouse" && event.button !== 0) return;
      event.preventDefault();
      event.stopPropagation();

      // 支持多指针时先结束旧拖拽
      const activeDrag = dragRef.current;
      if (activeDrag) finishResize(activeDrag.pointerId);

      const target = event.currentTarget;
      target.focus({ preventScroll: true });
      target.setPointerCapture(event.pointerId);
      dragRef.current = {
        pointerId: event.pointerId,
        startX: event.clientX,
        startWidth: widthRef.current,
        target,
        previousCursor: document.body.style.cursor,
        previousUserSelect: document.body.style.userSelect,
      };
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      setIsResizing(true);
    },
    [finishResize, widthRef],
  );

  const onPointerMove = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      const drag = dragRef.current;
      if (!drag || drag.pointerId !== event.pointerId) return;
      // 鼠标键已松开但未收到 up（罕见）时视为结束
      if (event.pointerType === "mouse" && event.buttons === 0) {
        finishResize(event.pointerId);
        return;
      }
      event.preventDefault();

      const direction = growthDirection === "right" ? 1 : -1;
      const nextWidth = clampWidth(drag.startWidth + (event.clientX - drag.startX) * direction);
      applyLiveWidth(nextWidth);
      event.currentTarget.setAttribute("aria-valuenow", String(nextWidth));
      event.currentTarget.setAttribute("aria-valuetext", `${nextWidth} px`);
    },
    [applyLiveWidth, clampWidth, finishResize, growthDirection],
  );

  const onPointerUp = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      finishResize(event.pointerId);
    },
    [finishResize],
  );

  const onPointerCancel = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      finishResize(event.pointerId);
    },
    [finishResize],
  );

  const onLostPointerCapture = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      finishResize(event.pointerId);
    },
    [finishResize],
  );

  /** 恢复默认宽度（双击 handle 触发；右栏用响应式默认） */
  const resetWidth = useCallback(() => {
    const nextDefault = getDefaultWidth?.() ?? defaultWidth;
    commitWidth(nextDefault, { forcePersist: true });
  }, [commitWidth, defaultWidth, getDefaultWidth]);

  /** 重新对当前宽度执行 clamp（窗口尺寸变化时防溢出） */
  const reclampWidth = useCallback(() => {
    commitWidth(widthRef.current);
  }, [commitWidth, widthRef]);

  /** 键盘调整（可访问性）：方向键 ±12/±32，Home/End 到 min/max，Enter 恢复默认 */
  const onKeyDown = useCallback(
    (event: KeyboardEvent<HTMLDivElement>) => {
      const step = event.shiftKey ? 32 : 12;
      const growKey = growthDirection === "right" ? "ArrowRight" : "ArrowLeft";
      const shrinkKey = growthDirection === "right" ? "ArrowLeft" : "ArrowRight";

      if (event.key === growKey) {
        event.preventDefault();
        commitWidth(widthRef.current + step, { forcePersist: true });
      } else if (event.key === shrinkKey) {
        event.preventDefault();
        commitWidth(widthRef.current - step, { forcePersist: true });
      } else if (event.key === "Home") {
        event.preventDefault();
        commitWidth(minWidth, { forcePersist: true });
      } else if (event.key === "End") {
        event.preventDefault();
        commitWidth(effectiveMaxWidth(), { forcePersist: true });
      } else if (event.key === "Enter") {
        event.preventDefault();
        resetWidth();
      }
    },
    [commitWidth, effectiveMaxWidth, growthDirection, minWidth, resetWidth, widthRef],
  );

  // 首次挂载：读取持久化宽度 → clamp 恢复（校验当前 viewport）；非法值则写回修正
  useEffect(() => {
    if (restoredRef.current) return;
    restoredRef.current = true;

    const storedWidth = readStoredWidth(storageKey);
    const candidate = storedWidth ?? getDefaultWidth?.() ?? defaultWidth;
    const restoredWidth = commitWidth(candidate, { persist: false });
    if (storedWidth !== null && storedWidth !== restoredWidth) {
      writeStoredWidth(storageKey, restoredWidth);
    }
  }, [commitWidth, defaultWidth, getDefaultWidth, storageKey]);

  // 挂载后监听窗口 resize：每次重新 clamp 当前宽度（防止布局溢出）
  useEffect(() => {
    if (!restoredRef.current) return;
    commitWidth(widthRef.current);

    const onResize = () => {
      commitWidth(widthRef.current);
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [commitWidth, widthRef]);

  // 拖拽期间窗口失焦/隐藏时强制结束，避免卡死拖拽态
  useEffect(() => {
    if (!isResizing) return;
    const cancelResize = () => {
      const drag = dragRef.current;
      if (drag) finishResize(drag.pointerId);
    };
    const onVisibilityChange = () => {
      if (document.visibilityState !== "visible") cancelResize();
    };
    window.addEventListener("blur", cancelResize);
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      window.removeEventListener("blur", cancelResize);
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  }, [finishResize, isResizing]);

  // 组件卸载时清理拖拽态
  useEffect(() => {
    return () => {
      const drag = dragRef.current;
      if (!drag) return;
      dragRef.current = null;
      restoreBodyState(drag);
    };
  }, [restoreBodyState]);

  return {
    isResizing,
    panelRef,
    reclampWidth,
    resetWidth,
    width,
    separatorProps: {
      "aria-label": ariaLabel,
      "aria-orientation": "vertical" as const,
      "aria-valuemax": effectiveMaxWidth(),
      "aria-valuemin": minWidth,
      "aria-valuenow": width,
      "aria-valuetext": `${width} px`,
      onDoubleClick: resetWidth,
      onKeyDown,
      onLostPointerCapture,
      onPointerCancel,
      onPointerDown,
      onPointerMove,
      onPointerUp,
      role: "separator" as const,
      tabIndex: 0,
    },
  };
}