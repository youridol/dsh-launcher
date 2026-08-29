// Tauri invoke 的类型化封装
// 对应 Rust 端 commands/* 模块（见 src-tauri/src/commands/）

import { invoke } from "@tauri-apps/api/core";

/** dsh 运行状态（对应 Rust DshStatus） */
export type DshStatus = "stopped" | "starting" | "running" | "stopping" | "error";

/** 工具链项（对应 Rust ToolchainItem） */
export interface ToolchainItem {
  name: string;
  installedVersion: string | null;
  required: string;
  state: string;
}

/** 可用 dsh 版本（对应 Rust DshVersion） */
export interface DshVersion {
  version: string;
  channel: "npm" | "github";
}

/** 查询 dsh 运行状态 */
export function getDshStatus(): Promise<DshStatus> {
  return invoke("get_status");
}

/** 启动 dsh web */
export function startDsh(): Promise<string> {
  return invoke("start_dsh");
}

/** 停止 dsh */
export function stopDsh(): Promise<string> {
  return invoke("stop_dsh");
}

/** 重启 dsh */
export function restartDsh(): Promise<string> {
  return invoke("restart_dsh");
}

/** 检测工具链 */
export function detectToolchain(): Promise<ToolchainItem[]> {
  return invoke("detect_toolchain");
}

/** 一键安装工具链 */
export function installToolchain(name: string): Promise<string> {
  return invoke("install_toolchain", { name });
}

/** 列出某通道可用版本 */
export function listVersions(channel: string): Promise<DshVersion[]> {
  return invoke("list_versions", { channel });
}

/** 获取已安装版本 */
export function getInstalledVersion(): Promise<string | null> {
  return invoke("get_installed_version");
}

/** 安装指定版本 */
export function installVersion(channel: string, version: string): Promise<string> {
  return invoke("install_version", { channel, version });
}

/** 卸载 dsh */
export function uninstallDsh(): Promise<string> {
  return invoke("uninstall");
}

/** 日志文件条目 */
export interface LogFile {
  path: string;
  size: number;
  modified: number;
}

/** 应用配置（对应 Rust ConfigView） */
export interface AppConfig {
  port: number;
  npmRegistry: string;
  githubMirror: string;
  nodeMirror: string;
  closeExits: boolean;
  minimizeToTray: boolean;
  keepDshOnExit: boolean;
  keepDshHomeOnUninstall: boolean;
  autoStartDsh: boolean;
  autoOpenBrowser: boolean;
}

/** 读取配置 */
export function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

/** 保存端口 */
export function setPort(port: number): Promise<void> {
  return invoke("set_port", { port });
}

/** 保存镜像源 */
export function setMirrors(opts: {
  npmRegistry: string;
  githubMirror: string;
  nodeMirror: string;
}): Promise<void> {
  return invoke("set_mirrors", {
    npmRegistry: opts.npmRegistry,
    githubMirror: opts.githubMirror,
    nodeMirror: opts.nodeMirror,
  });
}

/** 保存滑动开关 */
export function setSwitches(opts: {
  closeExits: boolean;
  minimizeToTray: boolean;
  keepDshOnExit: boolean;
  keepDshHomeOnUninstall: boolean;
  autoStartDsh: boolean;
  autoOpenBrowser: boolean;
}): Promise<void> {
  return invoke("set_switches", opts);
}


/** 列出日志文件 */
export function listLogs(): Promise<LogFile[]> {
  return invoke("list_logs");
}

/** 读取日志文件内容 */
export function readLog(relPath: string): Promise<string> {
  return invoke("read_log", { relPath });
}
