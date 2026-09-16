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

/**
 * 当前 dsh 是否由本启动器托管（v0.9.1）。
 *
 * dsh 的访问 token 是进程级随机数，只从该进程 stdout 打印。故"已运行但拿不到
 * token URL"有两种成因：托管实例只是**尚未打印**（继续等待即可）；收养的外部
 * 实例则**原理上不可得**（需询问用户是否接管）。
 */
export function isDshManaged(): Promise<boolean> {
  return invoke("is_dsh_managed");
}

/** 接管外部启动的 dsh（停止并以启动器方式重新拉起，从而能捕获 token） */
export function takeOverDsh(): Promise<string> {
  return invoke("take_over_dsh");
}

/** 获取 dsh web 完整访问 URL（含 token，免认证） */
export function getWebUrl(): Promise<string> {
  return invoke("get_web_url");
}

/** 探测 dsh web 是否 HTTP 200 可服务（开窗前确认，防冷启动 404） */
export function probeWebReady(url: string): Promise<boolean> {
  return invoke("probe_web_ready", { url });
}

/** 创建内嵌 Web GUI 窗口（v0.4.15：统一走 Rust 窗口创建，消除 JS 创建路径无高清图标的问题） */
export function createWebGuiWindow(url: string): Promise<string> {
  return invoke("create_web_gui_window", { url });
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
  /** v0.4.13：Rust 不再回传明文 token，只回传是否已设置 */
  githubTokenSet: boolean;
  nodeMirror: string;
  closeExits: boolean;
  minimizeToTray: boolean;
  keepDshOnExit: boolean;
  keepDshHomeOnUninstall: boolean;
  autoStartDsh: boolean;
  autoOpenBrowser: boolean;
  /** 启动后自动同步 upstream 插件（自研插件不受影响） */
  autoSyncPlugins: boolean;
  /** 外部编辑器命令（空 = 系统默认关联程序，见 ADR-0008） */
  editorCommand: string;
  /** 是否已问过「用哪个程序打开」（首次点击编辑时弹一次引导） */
  editorPromptSeen: boolean;
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
  autoSyncPlugins: boolean;
}): Promise<void> {
  return invoke("set_switches", opts);
}

/** 保存外部编辑器配置（ADR-0008） */
export function setEditor(opts: {
  editorCommand: string;
  promptSeen: boolean;
}): Promise<void> {
  return invoke("set_editor", {
    editorCommand: opts.editorCommand,
    promptSeen: opts.promptSeen,
  });
}


/** 列出日志文件 */
export function listLogs(): Promise<LogFile[]> {
  return invoke("list_logs");
}

/** 读取日志文件内容 */
export function readLog(relPath: string): Promise<string> {
  return invoke("read_log", { relPath });
}

// ==================== 插件管理（ADR-0005） ====================

/** 插件变更事件名（对应 Rust PLUGIN_CHANGED_EVENT） */
export const PLUGIN_CHANGED_EVENT = "plugin://changed";

/** 技能变更事件名（对应 Rust SKILL_CHANGED_EVENT）：管理侧启停/删除后广播 */
export const SKILL_CHANGED_EVENT = "skill://changed";

/** 订阅插件变更事件 */
export async function listenPluginChanged(
  onChanged: () => void,
): Promise<UnlistenFn> {
  return listen(PLUGIN_CHANGED_EVENT, () => {
    onChanged();
  });
}

/** 订阅技能变更事件（启停/删除后后端广播；前端据此重扫） */
export async function listenSkillChanged(
  onChanged: () => void,
): Promise<UnlistenFn> {
  return listen(SKILL_CHANGED_EVENT, () => {
    onChanged();
  });
}

/** 插件状态（对应 Rust PluginState） */
export type PluginState = "uninstalled" | "plain" | "enabled" | "disabled";

/** 行状态（对应 Rust RowState） */
export type RowState = "enabled" | "disabled" | "expression";

/** 来源分类（对应 Rust Origin） */
export type PluginOrigin = "upstream" | "in-house" | "unknown";

/** 依赖 spec 形态（对应 Rust SpecKind） */
export type SpecKind = "npm" | "git" | "path" | "tarball" | "unknown";

/** 插件来源描述 */
export interface PluginSource {
  kind: SpecKind;
  spec: string;
  repo: string | null;
  reference: string | null;
  commit: string | null;
}

/** 一次同步的结果 */
export interface SyncRecord {
  at: string;
  from: string;
  to: string;
  result: string;
}

/** 行视图 */
export interface RowView {
  id: string;
  name: string | null;
  state: RowState;
}

/** 插件视图（对应 Rust PluginView） */
export interface PluginView {
  package: string;
  version: string | null;
  origin: PluginOrigin;
  state: PluginState;
  rows: RowView[];
  source: PluginSource | null;
  lastSync: SyncRecord | null;
  protected: boolean;
  needsReconcile: boolean;
  desired: string | null;
  lastError: string | null;
  installedSpec: string | null;
}

/** 插件列表结果 */
export interface PluginList {
  profile: string;
  plugins: PluginView[];
  degradedReason: string | null;
}

/** 操作结果 */
export interface OpResult {
  status: "changed" | "unchanged";
  restarted: boolean;
  message: string;
  package: string | null;
}

/** 单个插件的同步结果 */
export interface SyncItemResult {
  package: string;
  origin: PluginOrigin;
  from: string | null;
  to: string | null;
  result: string;
  message: string;
}

/** 同步报告 */
export interface SyncReport {
  applied: boolean;
  restarted: boolean;
  items: SyncItemResult[];
}

/** 列出插件 */
export function pluginList(): Promise<PluginList> {
  return invoke("plugin_list");
}

/** 安装插件（spec 可为 npm 包名/git 地址/本地路径） */
export function pluginInstall(
  spec: string,
  origin?: PluginOrigin,
): Promise<OpResult> {
  return invoke("plugin_install", { spec, origin: origin ?? null });
}

/** 启用/禁用插件 */
export function pluginSetState(
  pkg: string,
  enabled: boolean,
): Promise<OpResult> {
  return invoke("plugin_set_state", { package: pkg, enabled });
}

/** 卸载插件 */
export function pluginUninstall(pkg: string): Promise<OpResult> {
  return invoke("plugin_uninstall", { package: pkg });
}

/** 同步 upstream 插件（apply=false 只检查） */
export function pluginSync(apply: boolean, pkg?: string): Promise<SyncReport> {
  return invoke("plugin_sync", { apply, package: pkg ?? null });
}

/** 收敛：重新对账 bundles 并重放期望态 */
/**
 * 修复 profile patch 配置文件（BUG-2，v0.9.8）。
 * 当 cordis.patch.yml 被外部工具写坏导致 dsh 无法启动/枚举插件时调用：
 * 自动备份原文件后把骨架重建为合法形态。
 */
export function pluginHealConfig(): Promise<string> {
  return invoke("plugin_heal_config");
}

export function pluginRepair(pkg?: string): Promise<OpResult> {
  return invoke("plugin_repair", { package: pkg ?? null });
}

// ==================== 技能管理（ADR-0007） ====================
//
// 数据来源是**文件系统扫描**（官方 `skills/list` Remote 只读且不含路径，无法用于
// 定位文件）。因此全部操作只依赖文件系统，**与 dsh 运行状态完全无关**。

/** 技能可用状态（对应 Rust SkillState） */
export type SkillState = "enabled" | "disabled" | "conflict" | "unreadable";

/** 一条受管技能（对应 Rust SkillEntry） */
export interface SkillEntry {
  /** 绝对路径：**身份键**，写操作回传该值作身份声明 */
  path: string;
  /** frontmatter 的 name */
  name: string;
  /** frontmatter 的 description */
  description: string;
  /** frontmatter 的 whenToUse */
  whenToUse?: string | null;
  /** 可用状态；`conflict` / `unreadable` 为只读，开关须禁用 */
  state: SkillState;
  /** 根标识：`user-dsh`（rank 400）/ `user-agents`（rank 500） */
  source: string;
  /** 官方 rank（400 / 500） */
  rank: number;
  /** 目录包 `<name>/SKILL.md` 还是平铺 `<name>.md` */
  bundled: boolean;
  /** 不可用时的具名原因 */
  reason?: string | null;
  /** 被同名更高 rank 技能覆盖时，覆盖者的名字 */
  overriddenBy?: string | null;
  /** 是否符号链接（链接技能禁止删除） */
  isSymlink: boolean;
  /** 所在根是否存在 */
  rootExists: boolean;
}

/** 受管根状态 */
export interface SkillRootStatus {
  path: string;
  source: string;
  rank: number;
  exists: boolean;
  skillCount: number;
}

/** 技能列表（对应 Rust SkillList） */
export interface SkillList {
  skills: SkillEntry[];
  roots: SkillRootStatus[];
  disabledCount: number;
  conflictCount: number;
  unreadableCount: number;
  trashCount: number;
  backupRoot: string;
}

/** 启停结果 */
export interface SkillToggleReport {
  /** false = 已是目标状态（幂等空操作，未写盘） */
  changed: boolean;
  message: string;
  backup?: string | null;
}

/** 删除结果 */
export interface SkillDeleteReport {
  trashedTo: string;
  message: string;
}

/** 列出全部受管技能（只读） */
export function skillList(): Promise<SkillList> {
  return invoke("skill_list");
}

/**
 * 启用/停用技能。
 *
 * `path` 与 `name` 是身份声明（ADR-0007 D10）：后端写前会重新扫描，要求磁盘上该路径的
 * frontmatter `name` 与之相符，否则拒绝并提示刷新。
 */
export function skillSetEnabled(
  path: string,
  name: string,
  enabled: boolean,
): Promise<SkillToggleReport> {
  return invoke("skill_set_enabled", { path, name, enabled });
}

/** 删除技能（移入 `<root>/.trash/`，可恢复） */
export function skillDelete(
  path: string,
  name: string,
): Promise<SkillDeleteReport> {
  return invoke("skill_delete", { path, name });
}

// ==================== 技能导入 / 更新 / 打开（ADR-0008） ====================

/** 单文件差异类型（对应 Rust FileChange） */
export type FileChange = "added" | "updated" | "same" | "localOnly" | "removed";

/** 单文件差异 */
export interface SkillFileDiff {
  path: string;
  change: FileChange;
}

/** 单个技能的导入计划（对应 Rust SkillImportPlan） */
export interface SkillImportPlan {
  name: string;
  repoPath: string;
  targetPath: string;
  actionable: boolean;
  added: number;
  updated: number;
  same: number;
  localOnly: number;
  removedUpstream: number;
  files: SkillFileDiff[];
  skipReason?: string | null;
}

/** 导入/检查结果（对应 Rust ImportReport） */
export interface SkillImportReport {
  url: string;
  commit?: string | null;
  plans: SkillImportPlan[];
  applied: boolean;
  message: string;
}

/** 从 git URL 导入技能（apply=false 为只读预览） */
export function skillImportUrl(
  url: string,
  apply: boolean,
): Promise<SkillImportReport> {
  return invoke("skill_import_url", { url, apply });
}

/** 批量导入的一条待导入条目（仓库标签 + URL） */
export interface SkillImportItem {
  name: string;
  url: string;
}

/** 批量导入的单条结果 */
export interface SkillBatchItemResult {
  name: string;
  url: string;
  ok: boolean;
  message: string;
  plans: SkillImportPlan[];
  commit?: string | null;
}

/** 批量导入聚合报告 */
export interface SkillBatchImportReport {
  items: SkillBatchItemResult[];
  applied: boolean;
  okCount: number;
  failedCount: number;
  message: string;
}

/** 批量导入多个仓库（apply=false 为只读预览；单条失败不中断其余） */
export function skillImportBatch(
  items: SkillImportItem[],
  apply: boolean,
): Promise<SkillBatchImportReport> {
  return invoke("skill_import_batch", { items, apply });
}

/** 一个来源的检查结果（对应 Rust SourceCheck） */
export interface SkillSourceCheck {
  url: string;
  recordedCommit?: string | null;
  remoteCommit?: string | null;
  commitChanged: boolean;
  plans: SkillImportPlan[];
  actionableCount: number;
  localOnlyCount: number;
  error?: string | null;
}

/** 全部来源的检查结果（对应 Rust UpdateCheckReport） */
export interface SkillUpdateCheckReport {
  sources: SkillSourceCheck[];
  actionableTotal: number;
  sourceCount: number;
  message: string;
}

/** 检查已登记来源的更新（纯只读，永不自动写盘） */
export function skillCheckUpdates(): Promise<SkillUpdateCheckReport> {
  return invoke("skill_check_updates");
}

/** 应用某个来源的更新（需用户确认后调用） */
export function skillApplyUpdate(url: string): Promise<SkillImportReport> {
  return invoke("skill_apply_update", { url });
}

/** 一个已导入来源（对应 Rust SourceRecord） */
export interface SkillSourceRecord {
  /** 仓库标签（批量导入时填写，可为空则回退到 URL） */
  name?: string | null;
  url: string;
  commit?: string | null;
  importedAt: string;
  checkedAt?: string | null;
  lastUpdateCount: number;
  skills: string[];
}

/** 技能来源注册表（对应 Rust SourceRegistry） */
export interface SkillSourceRegistry {
  schemaVersion: number;
  sources: SkillSourceRecord[];
}

/** 读取来源注册表 */
export function skillSources(): Promise<SkillSourceRegistry> {
  return invoke("skill_sources");
}

/** 移除某个来源记录（只删元数据，不动技能文件） */
export function skillForgetSource(url: string): Promise<void> {
  return invoke("skill_forget_source", { url });
}

/** 打开结果（对应 Rust OpenReport） */
export interface SkillOpenReport {
  path: string;
  via: "editor" | "system";
  created: boolean;
}

/**
 * 用外部程序打开受管文件。
 *
 * 二选一：`target` 是闭集枚举（"agents-md" | "context-md" | "skills-root"），
 * 路径由 Rust 推导；`path` 是某个受管技能文件，由 Rust 校验其仍在受管根内。
 * 前端**永远无法**让后端打开任意路径。
 */
export function skillOpen(
  target: "agents-md" | "context-md" | "skills-root",
): Promise<SkillOpenReport>;
export function skillOpen(path: string): Promise<SkillOpenReport>;
export function skillOpen(arg: string): Promise<SkillOpenReport> {
  const isTarget =
    arg === "agents-md" || arg === "context-md" || arg === "skills-root";
  return invoke("skill_open", {
    target: isTarget ? arg : null,
    path: isTarget ? null : arg,
  });
}

// ==================== MCP server 管理（ADR-0006） ====================

/** MCP 变更事件名（对应 Rust MCP_CHANGED_EVENT） */
export const MCP_CHANGED_EVENT = "mcp://changed";

/** 订阅 MCP 变更事件 */
export async function listenMcpChanged(
  onChanged: () => void,
): Promise<UnlistenFn> {
  return listen(MCP_CHANGED_EVENT, () => {
    onChanged();
  });
}

/** MCP server 三态（对应 Rust McpState） */
export type McpState = "missing" | "enabled" | "disabled";

/** 声明来源（对应 Rust McpOrigin） */
export type McpOrigin = "managed" | "external";

/** 行级只读标记（对应 Rust McpMark） */
export type McpMark = "expression" | "conflict" | "dangerous";

/** transport（官方字段的两个取值） */
export type McpTransport = "stdio" | "streamable-http";

/** `list.reload`：由 profile 的 `patchReload` 决定 */
export type McpReload = "live" | "requires-restart";

/** 单个 MCP server 视图（对应 Rust McpServerView） */
export interface McpServerView {
  serverName: string;
  rowId: string;
  transport: McpTransport | null;
  state: McpState;
  origin: McpOrigin;
  /** 来源层：dump 段标签（绝对路径 / bundle 包名） */
  layer: string;
  /** 只读摘要：stdio 取 command + args，streamable-http 取 url */
  summary: string;
  /** 有效 disabled；null = `!!js` 表达式（只读，启动器拒绝覆盖） */
  disabled: boolean | null;
  marks: McpMark[];
}

/** MCP 前置（`@deepseek-ai/dsh-mcp-client` 可解析性） */
export interface McpPrereq {
  installed: boolean;
  package: string;
}

/** MCP 列表结果（对应 Rust McpListResult） */
export interface McpListResult {
  profile: string;
  reload: McpReload;
  prereq: McpPrereq;
  servers: McpServerView[];
}

/** 新增 MCP server 的结构化入参（对应 Rust McpAddSpec） */
export interface McpAddSpec {
  serverName: string;
  transport: McpTransport;
  rowId?: string | null;
  startDisabled?: boolean;
  /** stdio */
  command?: string | null;
  args?: string[];
  env?: [string, string][];
  cwd?: string | null;
  /** streamable-http */
  url?: string | null;
  headers?: [string, string][];
  /** 共同可选项（官方字段名） */
  toolCallTimeoutMs?: number | null;
  reconnectEnabled?: boolean | null;
  reconnectInitialDelayMs?: number | null;
  reconnectMaxDelayMs?: number | null;
  reconnectMaxAttempts?: number | null;
  /**
   * 原始通道：官方 config 体的原始 YAML 片段（与结构化字段互斥）。
   * 用于 `!!js` / 注释 / 暂未建模的官方字段透传。
   */
  rawConfig?: string | null;
}

/** MCP 操作结果（`restarted` 恒为 false：MCP 变更不重启 dsh） */
export interface McpOpResult {
  status: "changed" | "unchanged";
  restarted: boolean;
  message: string;
  serverName: string | null;
}

/** 列出合成树全量 MCP server */
export function mcpList(): Promise<McpListResult> {
  return invoke("mcp_list");
}

/** 新增 MCP server（写机器级受管 MCP 区块） */
export function mcpAdd(spec: McpAddSpec): Promise<McpOpResult> {
  return invoke("mcp_add", { spec });
}

/** 删除 MCP server（managed 真删除；external 仅撤销定向覆盖） */
export function mcpRemove(serverName: string): Promise<McpOpResult> {
  return invoke("mcp_remove", { serverName });
}

/** 启用/禁用 MCP server（只写定向行，config 绝不重渲染） */
export function mcpSetState(
  serverName: string,
  enabled: boolean,
): Promise<McpOpResult> {
  return invoke("mcp_set_state", { serverName, enabled });
}
