// 安装进度事件封装：订阅 Rust 端 install://progress 事件
// 对应 src-tauri/src/core/events.rs

import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** 安装阶段（对应 Rust InstallPhase） */
export type InstallPhase = "prepare" | "download" | "install" | "build" | "done";

/** 进度事件载荷（对应 Rust ProgressPayload） */
export interface InstallProgress {
  channel: string;
  phase: InstallPhase;
  percent: number;
  message: string;
}

/** 安装进度事件名（对应 Rust PROGRESS_EVENT） */
export const PROGRESS_EVENT = "install://progress";

/** 订阅安装进度事件，返回取消订阅函数 */
export async function listenProgress(
  onProgress: (p: InstallProgress) => void,
): Promise<UnlistenFn> {
  return listen<InstallProgress>(PROGRESS_EVENT, (event) => {
    onProgress(event.payload);
  });
}

/** 阶段中文标签（进度条 UI 用） */
export const PHASE_LABELS: Record<InstallPhase, string> = {
  prepare: "准备中",
  download: "下载中",
  install: "安装依赖",
  build: "构建中",
  done: "已完成",
};
