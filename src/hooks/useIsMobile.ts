// 移动端断点检测（≤640px），与 index.css 的响应式断点保持一致
import { useSyncExternalStore } from "react";
import { MOBILE_MAX_WIDTH } from "@/lib/panel-layout";

// G7（审计 §2.1）：断点值单一来源（`panel-layout.ts`），不再本地硬编码。
const MOBILE_QUERY = `(max-width: ${MOBILE_MAX_WIDTH}px)`;

function subscribe(cb: () => void): () => void {
  if (typeof window === "undefined" || !window.matchMedia) return () => {};
  const mql = window.matchMedia(MOBILE_QUERY);
  mql.addEventListener("change", cb);
  return () => mql.removeEventListener("change", cb);
}

function getSnapshot(): boolean {
  if (typeof window === "undefined" || !window.matchMedia) return false;
  return window.matchMedia(MOBILE_QUERY).matches;
}

function getServerSnapshot(): boolean {
  return false;
}

/** 视口 ≤640px 时返回 true */
export function useIsMobile(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
}