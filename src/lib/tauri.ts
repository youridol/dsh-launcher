// Tauri invoke 的类型化封装
// 对应 Rust 端 commands/* 模块（见 src-tauri/src/commands/）

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** dsh 安装版本变更事件名（对应 Rust VERSION_CHANGED_EVENT） */
export const VERSION_CHANGED_EVENT = "version://changed";

/** 订阅 dsh 安装版本变更事件（卸载/安装后触发，前端刷新安装状态） */
export async function listenVersionChanged(
  onChanged: () => void,
): Promise<UnlistenFn> {
  return listen(VERSION_CHANGED_EVENT, () => {
    onChanged();
  });
}

/** 工具链变更事件名（对应 Rust TOOLCHAIN_CHANGED_EVENT） */
export const TOOLCHAIN_CHANGED_EVENT = "toolchain://changed";

/** 订阅工具链变更事件（安装完成后触发，前端刷新各条目状态） */
export async function listenToolchainChanged(
  onChanged: () => void,
): Promise<UnlistenFn> {
  return listen(TOOLCHAIN_CHANGED_EVENT, () => {
    onChanged();
  });
}

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

/** 获取 dsh web 完整访问 URL（含 token，免认证） */
export function getWebUrl(): Promise<string> {
  return invoke("get_web_url");
}

/** 为内嵌 Web GUI 窗口设置高清任务栏图标（修复图标模糊） */
export function setWebGuiIcon(label: string): Promise<void> {
  return invoke("set_web_gui_icon", { label });
}

/** 创建桌面快捷方式（双击用默认浏览器打开 dsh web） */
export function createDesktopShortcut(): Promise<string> {
  return invoke("create_desktop_shortcut");
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

/** 卸载工具链（node 同步删目录；git/python 启动官方卸载器 UAC） */
export function uninstallToolchain(name: string): Promise<string> {
  return invoke("uninstall_toolchain", { name });
}

/** 批量操作结果（对应 Rust BatchResult） */
export interface BatchResult {
  name: string;
  ok: boolean;
  message: string;
}

/** 批量一键安装缺失工具链（按 node→pnpm→git→python 依赖顺序） */
export function batchInstallToolchains(): Promise<BatchResult[]> {
  return invoke("batch_install_toolchains");
}

/** 一键卸载全部工具链 */
export function batchUninstallToolchains(): Promise<BatchResult[]> {
  return invoke("batch_uninstall_toolchains");
}

/** 列出某通道可用版本 */
export function listVersions(channel: string): Promise<DshVersion[]> {
  return invoke("list_versions", { channel });
}

/** 获取已安装版本 */
export function getInstalledVersion(): Promise<string | null> {
  return invoke("get_installed_version");
}

/** 安装路径信息 */
export interface InstallPaths {
  githubDir: string;
  githubInstalled: boolean;
  npmGlobalDir: string;
  npmBinDir: string;
}

/** 获取 harness 下载/安装目录 */
export function getInstallPaths(): Promise<InstallPaths> {
  return invoke("get_install_paths");
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
  githubToken: string;
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

/** 保存 GitHub Token（防 API 限流 / git 认证增强） */
export function setGithubToken(token: string): Promise<void> {
  return invoke("set_github_token", { token });
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
