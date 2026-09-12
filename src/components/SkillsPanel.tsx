// 技能管理面板（ADR-0007 / ADR-0008）：
// - 只管理**用户级**技能根：`<DSH_HOME>/skills`（官方 rank 400）与
//   `<agentsHome>/skills`（官方 rank 500）。项目根取决于 dsh 会话工作区、custom 根由
//   preset 声明，二者对启动器不可知/不可见，故一律不碰（ADR-0007 D5）。
// - 单一滑动开关 = frontmatter `disable-model-invocation`（官方语义：缺省即允许；
//   仅显式 `true` 关闭模型面）。**停用 ≠ 不可用** —— 用户在 dsh 里仍可 `/名称` 调用。
// - 全部操作**只依赖文件系统**，与 dsh 是否运行完全无关；官方 watcher 会在 dsh 运行中
//   热重载（~200ms 稳定后失效缓存），未运行时下次启动自然生效，**无需重启**。
// - 身份键是**绝对路径**：后端写前重扫校验「路径仍在受管根内 ∧ 磁盘 name 与面板声明一致」，
//   不符即拒绝并提示刷新（防「面板打开后技能被改名，开关误伤同名技能」）。
// - 只读状态（冲突/无法解析）**禁用开关**，并显示具名原因 —— 绝不猜测用户文件含义。
// - 布局（ADR-0008 修订）：**批量导入 + 检查更新放在面板顶部**，技能列表居中 ——
//   否则 93 个技能的列表会把导入/更新埋到最底部，用户根本看不到（v0.9.0 实测问题）。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRefreshOnEvent } from "@/hooks/useTauriEvent";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  getConfig,
  listenSkillChanged,
  setEditor,
  skillApplyUpdate,
  skillCheckUpdates,
  skillDelete,
  skillForgetSource,
  skillImportBatch,
  skillList,
  skillOpen,
  skillSetEnabled,
  skillSources,
  type SkillBatchImportReport,
  type SkillEntry,
  type SkillList,
  type SkillSourceRegistry,
  type SkillState,
  type SkillUpdateCheckReport,
} from "@/lib/tauri";
import { toast } from "sonner";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  ChevronRight,
  ExternalLink,
  FilePen,
  FolderOpen,
  Loader2,
  RefreshCw,
  Trash2,
} from "lucide-react";

/** 状态徽章配色 */
function stateVariant(state: SkillState) {
  switch (state) {
    case "enabled":
      return "default" as const;
    case "disabled":
      return "secondary" as const;
    case "conflict":
    case "unreadable":
      return "destructive" as const;
    default:
      return "outline" as const;
  }
}

/** 状态中文标签 */
function stateLabel(state: SkillState) {
  switch (state) {
    case "enabled":
      return "启用";
    case "disabled":
      return "已停用";
    case "conflict":
      return "冲突";
    case "unreadable":
      return "无法解析";
    default:
      return state;
  }
}

/** 根来源的中文名（含官方 rank） */
function rootLabel(source: string) {
  switch (source) {
    case "user-dsh":
      return "DSH_HOME 技能根（rank 400）";
    case "user-agents":
      return "agentsHome 技能根（rank 500）";
    default:
      return source;
  }
}

export default function SkillsPanel() {
  const [data, setData] = useState<SkillList | null>(null);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [pendingDelete, setPendingDelete] = useState<SkillEntry | null>(null);
  const mounted = useRef(true);

  // ==================== ADR-0008：批量导入 / 检查更新 / 外部打开 ====================
  const [pendingRepos, setPendingRepos] = useState<
    { name: string; url: string }[]
  >([]);
  const [repoNameDraft, setRepoNameDraft] = useState("");
  const [repoUrlDraft, setRepoUrlDraft] = useState("");
  const [importPreview, setImportPreview] =
    useState<SkillBatchImportReport | null>(null);
  const [importing, setImporting] = useState(false);
  const [sources, setSources] = useState<SkillSourceRegistry | null>(null);
  const [checkReport, setCheckReport] =
    useState<SkillUpdateCheckReport | null>(null);
  const [checking, setChecking] = useState(false);
  const [editorPromptOpen, setEditorPromptOpen] = useState(false);
  const [editorCommandDraft, setEditorCommandDraft] = useState("");

  const refresh = useCallback(async () => {
    try {
      const next = await skillList();
      if (!mounted.current) return;
      setData(next);
    } catch (e) {
      toast.error(`读取技能列表失败: ${e}`);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    refresh();
    return () => {
      mounted.current = false;
    };
  }, [refresh]);

  // 订阅 skill://changed（导入/更新/启停/删除后由 Rust 广播）
  // 统一走 useTauriEvent（ADR-0009 D7）：此前用 `.then()` 无 `.catch` 且无竞态保护
  useRefreshOnEvent(listenSkillChanged, () => refresh(), [refresh]);

  useEffect(() => {
    skillSources().then(setSources).catch(() => {});
  }, []);

  const filtered = useMemo(() => {
    const all = data?.skills ?? [];
    const q = query.trim().toLowerCase();
    if (!q) return all;
    return all.filter(
      (s) =>
        s.name.toLowerCase().includes(q) ||
        s.description.toLowerCase().includes(q),
    );
  }, [data, query]);

  async function run(key: string, action: () => Promise<string>) {
    setBusy(key);
    try {
      const message = await action();
      await refresh();
      if (message) toast.success(message);
    } catch (e) {
      toast.error(String(e));
      await refresh();
    } finally {
      if (mounted.current) setBusy(null);
    }
  }

  async function toggle(skill: SkillEntry, enabled: boolean) {
    await run(`toggle:${skill.path}`, async () => {
      const report = await skillSetEnabled(skill.path, skill.name, enabled);
      return report.changed ? "" : report.message;
    });
  }

  async function confirmDelete() {
    const target = pendingDelete;
    if (!target) return;
    setPendingDelete(null);
    await run(`delete:${target.path}`, async () => {
      const report = await skillDelete(target.path, target.name);
      return report.message;
    });
  }

  function toggleExpand(path: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  async function reveal(skill: SkillEntry) {
    try {
      await revealItemInDir(skill.path);
    } catch (e) {
      toast.error(`定位失败: ${e}`);
    }
  }

  function addPendingRepo() {
    const url = repoUrlDraft.trim();
    if (!url) {
      toast.error("请输入 github 仓库 URL");
      return;
    }
    if (pendingRepos.some((r) => r.url === url)) {
      toast.error("该仓库已在待导入列表中");
      return;
    }
    setPendingRepos((prev) => [...prev, { name: repoNameDraft.trim(), url }]);
    setRepoUrlDraft("");
    setRepoNameDraft("");
  }

  function removePendingRepo(url: string) {
    setPendingRepos((prev) => prev.filter((r) => r.url !== url));
  }

  async function previewImport() {
    if (pendingRepos.length === 0) {
      toast.error("请先「增加」至少一个仓库条目");
      return;
    }
    setImporting(true);
    try {
      const report = await skillImportBatch(pendingRepos, false);
      setImportPreview(report);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setImporting(false);
    }
  }

  async function confirmImport() {
    if (pendingRepos.length === 0) return;
    setImporting(true);
    try {
      const report = await skillImportBatch(pendingRepos, true);
      toast.success(report.message);
      setImportPreview(null);
      setPendingRepos([]);
      await refresh();
      setSources(await skillSources());
    } catch (e) {
      toast.error(String(e));
    } finally {
      setImporting(false);
    }
  }

  async function openManaged(
    target: "agents-md" | "context-md" | "skills-root" | "skill",
    path?: string,
  ) {
    try {
      const cfg = await getConfig();
      if (!cfg.editorPromptSeen) {
        setEditorPromptOpen(true);
        setEditorCommandDraft(cfg.editorCommand);
        return;
      }
      if (target === "skill" && path) {
        const report = await skillOpen(path);
        if (report.created) toast.success(`已创建并打开 ${report.path}`);
        return;
      }
      const report = await skillOpen(
        target as "agents-md" | "context-md" | "skills-root",
      );
      if (report.created) toast.success(`已创建并打开 ${report.path}`);
    } catch (e) {
      toast.error(`打开失败: ${e}`);
    }
  }

  async function commitEditorChoice(useSystem: boolean) {
    const command = useSystem ? "" : editorCommandDraft.trim();
    try {
      await setEditor({ editorCommand: command, promptSeen: true });
      setEditorPromptOpen(false);
      toast.success(
        useSystem ? "已设为系统默认程序打开" : `已设为编辑器 ${command || "(空)"}`,
      );
    } catch (e) {
      toast.error(`保存编辑器配置失败: ${e}`);
    }
  }

  async function checkUpdates() {
    setChecking(true);
    try {
      const report = await skillCheckUpdates();
      setCheckReport(report);
      setSources(await skillSources());
      toast.success(report.message);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setChecking(false);
    }
  }

  /**
   * 移除一条来源记录（ADR-0008：**只删元数据，不动技能文件**）。
   *
   * 此前后端命令 `skill_forget_source` 与 TS 封装均存在但**无 UI 入口**
   * （审计 G2 ④），导致来源清单只增不减。此处接到已有的来源列表上。
   */
  async function forgetSource(url: string, label: string) {
    if (
      !window.confirm(
        `仅从来源记录中移除「${label}」？\n\n已导入的技能文件不会被删除，也不会卸载。`,
      )
    ) {
      return;
    }
    try {
      await skillForgetSource(url);
      toast.success(`已移除来源记录「${label}」（技能文件未删除）`);
      setSources(await skillSources());
      // 检查报告里可能仍引用了该来源 → 一并清掉，避免显示过期条目
      setCheckReport(null);
    } catch (e) {
      toast.error(String(e));
    }
  }

  async function applyUpdate(url: string) {
    setChecking(true);
    try {
      const report = await skillApplyUpdate(url);
      toast.success(report.message);
      await refresh();
      setCheckReport(null);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setChecking(false);
    }
  }

  const roots = data?.roots ?? [];
  const anyRoot = roots.some((r) => r.exists);

  const winnerRankByName = useMemo(() => {
    const map = new Map<string, number>();
    for (const s of data?.skills ?? []) {
      if (s.overriddenBy === null || s.overriddenBy === undefined) {
        map.set(s.name, s.rank);
      }
    }
    return map;
  }, [data]);

  return (
    <div className="space-y-4">
      {/* 受管根概览 */}
      <div className="space-y-1 text-xs">
        {roots.map((root) => (
          <div key={root.source} className="flex flex-wrap items-center gap-1.5">
            <Badge variant={root.exists ? "outline" : "secondary"}>
              {root.exists ? `${root.skillCount} 个` : "不存在"}
            </Badge>
            <span className="text-muted-foreground">{rootLabel(root.source)}</span>
            <span className="dsh-code min-w-0 flex-1 truncate">{root.path}</span>
          </div>
        ))}
        <div className="flex flex-wrap items-center gap-1">
          <Badge variant="outline">技能 {data?.skills.length ?? 0} 个</Badge>
          <Badge variant="secondary">已停用 {data?.disabledCount ?? 0} 个</Badge>
          {(data?.conflictCount ?? 0) > 0 && (
            <Badge variant="destructive">冲突 {data?.conflictCount}</Badge>
          )}
          {(data?.unreadableCount ?? 0) > 0 && (
            <Badge variant="destructive">无法解析 {data?.unreadableCount}</Badge>
          )}
          {(data?.trashCount ?? 0) > 0 && (
            <Badge variant="outline">回收站 {data?.trashCount} 项</Badge>
          )}
        </div>
      </div>

      {!anyRoot && (
        <p className="text-[11px] leading-relaxed text-muted-foreground">
          本机没有用户级技能根。技能目录创建后（例如
          <code className="dsh-code">&lt;agentsHome&gt;/skills/&lt;名称&gt;/SKILL.md</code>
          ）刷新即可管理。
        </p>
      )}

      {/* 共享资源（AGENTS.md / CONTEXT.md）—— 置于顶部第二行（不置底） */}
      <Separator />
      <div className="space-y-2">
        <div className="text-sm font-medium">共享资源（agentsHome）</div>
        <p className="text-[11px] leading-relaxed text-muted-foreground">
          这两个文件位于共享真源 <code className="dsh-code">&lt;agentsHome&gt;</code>。
          <code className="dsh-code">AGENTS.md</code> 是 dsh 固定读取的全局指令真源；
          <code className="dsh-code">CONTEXT.md</code> 是 agent 侧约定的词表（dsh 不读）。
          点击编辑用外部编辑器 / 系统默认程序打开；文件不存在时自动按模板创建。
        </p>
        <div className="flex flex-wrap items-center gap-1.5">
          <Button
            variant="outline"
            size="sm"
            disabled={busy !== null}
            onClick={() => openManaged("agents-md")}
          >
            <FilePen className="size-3" /> 编辑 AGENTS.md
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={busy !== null}
            onClick={() => openManaged("context-md")}
          >
            <FilePen className="size-3" /> 编辑 CONTEXT.md
          </Button>
          <Button
            variant="ghost"
            size="sm"
            disabled={busy !== null}
            onClick={() => openManaged("skills-root")}
            title="用编辑器 / 系统默认程序打开技能根目录"
          >
            <ExternalLink className="size-3" /> 技能根目录
          </Button>
        </div>
      </div>

      <Separator />

      {/* ==================== 批量导入 github 仓库（置顶） ==================== */}
      <div className="space-y-2">
        <div className="text-sm font-medium">批量导入 github 仓库</div>
        <p className="text-[11px] leading-relaxed text-muted-foreground">
          填写<strong>仓库名</strong>（可选，作标签）与<strong>仓库 URL</strong>，点「增加」
          追加到待导入列表；可一次添加多个仓库，点「确定」批量导入。启动器会逐条浅克隆、
          <strong>递归收集所有 SKILL.md</strong> 并按官方「扫描根只认一层」的规则扁平化到
          <code className="dsh-code">agentsHome/skills/</code>；目录名取 frontmatter 的
          <code className="dsh-code">name</code>，同名技能按<strong>文件级覆盖</strong>
          （上游有则覆盖、本地独有保留、远程删除不落地）。
        </p>

        <div className="flex flex-wrap items-center gap-1.5">
          <Input
            className="h-7 w-32 shrink-0 text-xs sm:w-40"
            placeholder="仓库名（可选）"
            value={repoNameDraft}
            onChange={(e) => setRepoNameDraft(e.target.value)}
          />
          <Input
            className="h-7 min-w-52 flex-1 text-xs"
            placeholder="https://github.com/owner/repo"
            value={repoUrlDraft}
            onChange={(e) => setRepoUrlDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") addPendingRepo();
            }}
          />
          <Button
            variant="outline"
            size="sm"
            disabled={importing || !repoUrlDraft.trim()}
            onClick={addPendingRepo}
          >
            增加
          </Button>
        </div>

        {pendingRepos.length > 0 && (
          <div className="space-y-1">
            {pendingRepos.map((r) => (
              <div
                key={r.url}
                className="dsh-list-row flex flex-wrap items-center gap-1.5 p-2"
              >
                <Badge variant="outline">
                  {r.name || r.url.split("/").slice(-1)[0] || r.url}
                </Badge>
                <span className="dsh-code min-w-0 flex-1 truncate text-[11px]">
                  {r.url}
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={importing}
                  onClick={() => removePendingRepo(r.url)}
                >
                  移除
                </Button>
              </div>
            ))}
            <div className="flex flex-wrap items-center gap-1.5 pt-1">
              <Button
                variant="default"
                size="sm"
                disabled={importing}
                onClick={confirmImport}
                title="对列表中所有仓库执行导入"
              >
                {importing ? (
                  <Loader2 className="size-3 animate-spin" />
                ) : null}
                确定（批量导入 {pendingRepos.length} 个）
              </Button>
              <Button
                variant="outline"
                size="sm"
                disabled={importing}
                onClick={previewImport}
                title="先只读预览，不写盘"
              >
                预览
              </Button>
              <Button
                variant="ghost"
                size="sm"
                disabled={importing}
                onClick={() => setPendingRepos([])}
              >
                清空
              </Button>
            </div>
          </div>
        )}

        {importPreview && (
          <div className="dsh-list-row space-y-1.5 p-2">
            <div className="text-xs font-medium">
              {importPreview.applied ? "批量导入结果" : "批量预览"}（成功{" "}
              {importPreview.okCount} / 失败 {importPreview.failedCount}）
            </div>
            {importPreview.items.map((it) => (
              <div key={it.url} className="text-[11px] text-muted-foreground">
                {it.ok ? (
                  <>
                    <span className="text-foreground">{it.name}</span>：
                    {it.message}
                    {!importPreview.applied && (
                      <>
                        {" "}
                        {it.plans
                          .filter((p) => p.actionable)
                          .map((p) => `${p.name}(+${p.added}/~${p.updated})`)
                          .join(", ") || "（无需变更）"}
                      </>
                    )}
                  </>
                ) : (
                  <span className="text-destructive">
                    {it.name}：{it.message}
                  </span>
                )}
              </div>
            ))}
          </div>
        )}
      </div>

      {/* ==================== 检查更新（置顶） ==================== */}
      <Separator />
      <div className="space-y-2">
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="text-sm font-medium">检查更新</span>
          <Badge variant="outline">来源 {sources?.sources.length ?? 0} 个</Badge>
        </div>
        <p className="text-[11px] leading-relaxed text-muted-foreground">
          对「从 URL 导入」登记过的来源做<strong>手动</strong>检查（浅克隆 + 逐文件比内容）。
          结果分三类：新增 / 覆盖更新 / 本地独有保留。**检查永不自动写盘**，需你逐个确认后才应用。
        </p>
        <Button
          variant="outline"
          size="sm"
          disabled={checking || (sources?.sources.length ?? 0) === 0}
          onClick={checkUpdates}
        >
          {checking ? (
            <Loader2 className="size-3 animate-spin" />
          ) : (
            <RefreshCw className="size-3" />
          )}
          检查更新
        </Button>

        {(sources?.sources.length ?? 0) > 0 && (
          <div className="space-y-1 text-[11px] text-muted-foreground">
            {sources!.sources.map((s) => (
              <div key={s.url} className="dsh-list-row p-2">
                <div className="flex flex-wrap items-center gap-1.5">
                  <span className="min-w-0 flex-1 truncate font-medium">
                    {s.name || s.url}
                  </span>
                  <Badge variant="outline">{s.skills.length} 个技能</Badge>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-6 px-1.5 text-[11px] text-muted-foreground"
                    title="仅移除来源记录，不删除已导入的技能文件"
                    onClick={() => void forgetSource(s.url, s.name || s.url)}
                  >
                    <Trash2 className="size-3" />
                    移除记录
                  </Button>
                </div>
                {s.name && (
                  <div className="text-[11px] text-muted-foreground">
                    <span className="dsh-code break-all">{s.url}</span>
                  </div>
                )}
                <div className="text-[11px] text-muted-foreground">
                  导入 commit {s.commit?.slice(0, 7) ?? "?"} · 导入于{" "}
                  {s.importedAt
                    ? new Date(Number(s.importedAt) * 1000).toLocaleString()
                    : "?"}
                </div>
              </div>
            ))}
          </div>
        )}

        {checkReport && checkReport.sources.length > 0 && (
          <div className="space-y-1.5">
            {checkReport.sources.map((src) => (
              <div key={src.url} className="dsh-list-row space-y-1 p-2">
                <div className="flex flex-wrap items-center gap-1.5">
                  <span className="min-w-0 flex-1 truncate text-xs font-medium">
                    {src.url}
                  </span>
                  {src.error ? (
                    <Badge variant="destructive">检查失败</Badge>
                  ) : src.actionableCount === 0 ? (
                    <Badge variant="secondary">已是最新</Badge>
                  ) : (
                    <Badge variant="default">{src.actionableCount} 个待更新</Badge>
                  )}
                </div>
                {src.error ? (
                  <div className="text-[11px] text-destructive">{src.error}</div>
                ) : (
                  <>
                    {src.commitChanged && (
                      <div className="text-[11px] text-muted-foreground">
                        commit {src.recordedCommit?.slice(0, 7) ?? "?"} →{" "}
                        {src.remoteCommit?.slice(0, 7) ?? "?"}
                      </div>
                    )}
                    {src.plans
                      .filter((p) => p.actionable)
                      .map((p) => (
                        <div key={p.name} className="text-[11px] text-muted-foreground">
                          {p.name}：新增 {p.added} / 覆盖 {p.updated} / 保留本地{" "}
                          {p.localOnly}
                        </div>
                      ))}
                    {src.actionableCount > 0 && (
                      <Button
                        variant="default"
                        size="sm"
                        disabled={checking}
                        onClick={() => applyUpdate(src.url)}
                      >
                        应用更新（文件级覆盖，保留本地）
                      </Button>
                    )}
                  </>
                )}
              </div>
            ))}
          </div>
        )}
      </div>

      <Separator />

      {/* 工具条 */}
      <div className="flex flex-wrap items-center gap-1.5">
        <Input
          className="h-7 w-full min-w-40 flex-1 text-xs"
          placeholder="搜索技能名称或描述…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <Button
          variant="outline"
          size="sm"
          disabled={busy !== null}
          onClick={() => refresh()}
        >
          {busy === null ? (
            <RefreshCw className="size-3" />
          ) : (
            <Loader2 className="size-3 animate-spin" />
          )}
          刷新
        </Button>
      </div>

      <p className="text-[11px] leading-relaxed text-muted-foreground">
        开关写入技能文件 frontmatter 的{" "}
        <code className="dsh-code">disable-model-invocation</code>。停用后模型目录不再
        包含该技能，但你在 dsh 里输入{" "}
        <code className="dsh-code">/技能名</code> 仍可手动调用。改动由 dsh 热重载生效，
        <strong>无需重启</strong>，且与 dsh 是否运行无关。
      </p>

      {/* 技能列表 */}
      <div className="space-y-1.5">
        {filtered.length === 0 && (
          <div className="py-6 text-center text-xs text-muted-foreground">
            {query ? "没有匹配的技能" : "没有可管理的技能"}
          </div>
        )}
        {filtered.map((skill) => {
          const overridden =
            skill.overriddenBy !== null && skill.overriddenBy !== undefined;
          const writable =
            skill.state === "enabled" || skill.state === "disabled";
          const isOpen = expanded.has(skill.path);
          const collapsedByDefault = overridden && !isOpen;
          return (
            <div key={skill.path} className="dsh-list-row space-y-1.5 p-2">
              <div className="flex flex-wrap items-center gap-1.5">
                <button
                  type="button"
                  className="flex size-4 shrink-0 items-center justify-center text-muted-foreground transition-transform hover:text-foreground"
                  onClick={() => toggleExpand(skill.path)}
                  aria-label={isOpen ? "折叠" : "展开"}
                  title={isOpen ? "折叠" : "展开"}
                >
                  <ChevronRight
                    className={`size-3.5 transition-transform duration-150 ${
                      isOpen ? "rotate-90" : ""
                    }`}
                  />
                </button>
                <Badge variant={stateVariant(skill.state)}>
                  {stateLabel(skill.state)}
                </Badge>
                <span className="min-w-0 truncate text-xs font-medium">
                  {skill.name}
                </span>
                <Badge variant="outline">rank {skill.rank}</Badge>
                {!skill.bundled && <Badge variant="outline">平铺文件</Badge>}
                {skill.isSymlink && <Badge variant="secondary">链接</Badge>}
                {overridden && (
                  <Badge variant="secondary">
                    被 rank {winnerRankByName.get(skill.name) ?? "?"} 的同名技能覆盖
                  </Badge>
                )}
                <span className="flex-1" />
                <Switch
                  size="sm"
                  checked={skill.state === "enabled"}
                  disabled={!writable || busy !== null}
                  onCheckedChange={(v) => toggle(skill, v === true)}
                  aria-label={`${skill.name} 启用状态`}
                  title={
                    writable
                      ? skill.state === "enabled"
                        ? "点击停用（模型不再调用；仍可 /名称 手动调用）"
                        : "点击启用（重新进入模型目录）"
                      : "该技能状态不可安全改写，开关已禁用"
                  }
                />
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={busy !== null}
                  onClick={() => reveal(skill)}
                  title="在资源管理器中定位"
                >
                  <FolderOpen className="size-3" />
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={busy !== null}
                  onClick={() => openManaged("skill", skill.path)}
                  title="用编辑器 / 系统默认程序打开 SKILL.md"
                >
                  <FilePen className="size-3" />
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={busy !== null || skill.isSymlink}
                  onClick={() => setPendingDelete(skill)}
                  title={
                    skill.isSymlink
                      ? "符号链接技能不可删除（会移走链接目标）"
                      : "移入回收站（可恢复）"
                  }
                >
                  {busy === `delete:${skill.path}` ? (
                    <Loader2 className="size-3 animate-spin" />
                  ) : (
                    "删除"
                  )}
                </Button>
              </div>

              {collapsedByDefault ? (
                <div className="truncate pl-6 text-[11px] text-muted-foreground">
                  {skill.description || "（无描述）"}
                </div>
              ) : (
                <div className="space-y-1 pl-6">
                  <div className="text-[11px] leading-relaxed text-muted-foreground">
                    {skill.description || "（无描述）"}
                  </div>
                  {skill.whenToUse && (
                    <div className="text-[11px] leading-relaxed text-muted-foreground">
                      适用时机 {skill.whenToUse}
                    </div>
                  )}
                  <div className="text-[11px] text-muted-foreground">
                    <code className="dsh-code break-all">{skill.path}</code>
                  </div>
                  {skill.reason && (
                    <div className="text-[11px] text-destructive">
                      {skill.reason}
                    </div>
                  )}
                  {overridden && (
                    <div className="text-[11px] text-muted-foreground">
                      同名技能有两个副本，rank 更高的那个（rank{" "}
                      {winnerRankByName.get(skill.name) ?? "?"}）生效。停用本项对模型
                      无影响；要改变生效状态请操作生效的那一条。
                    </div>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* 首次编辑器引导 */}
      <Dialog
        open={editorPromptOpen}
        onOpenChange={(open) => {
          if (!open) setEditorPromptOpen(false);
        }}
      >
        <DialogContent className="w-[calc(100vw-2rem)] max-w-md">
          <DialogHeader>
            <DialogTitle>选择打开方式</DialogTitle>
            <DialogDescription>
              这是首次打开文件。选择用系统默认关联程序，或指定一个编辑器
              （如 <code className="dsh-code">code --wait</code>、
              <code className="dsh-code">notepad</code>）。之后可在设置面板随时修改。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-2">
            <Input
              className="h-7 text-xs"
              placeholder="留空 = 系统默认程序（可填 code、notepad、C:\…\Code.exe --wait）"
              value={editorCommandDraft}
              onChange={(e) => setEditorCommandDraft(e.target.value)}
            />
          </div>
          <DialogFooter>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => commitEditorChoice(true)}
            >
              用系统默认程序
            </Button>
            <Button
              variant="default"
              size="sm"
              onClick={() => commitEditorChoice(false)}
            >
              用指定编辑器打开
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 删除确认 */}
      <Dialog
        open={pendingDelete !== null}
        onOpenChange={(open) => {
          if (!open) setPendingDelete(null);
        }}
      >
        <DialogContent className="w-[calc(100vw-2rem)] max-w-md">
          <DialogHeader>
            <DialogTitle>移入回收站</DialogTitle>
            <DialogDescription>
              将「{pendingDelete?.name}」移入所属技能根的{" "}
              <code className="dsh-code">.trash/</code> 目录。该操作可恢复（把目录移回原位
              即可），不会删除技能内容。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" size="sm" onClick={() => setPendingDelete(null)}>
              取消
            </Button>
            <Button variant="destructive" size="sm" onClick={confirmDelete}>
              移入回收站
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
