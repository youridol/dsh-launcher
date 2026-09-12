// 插件管理面板（ADR-0005）：
// - 按 ID（包名）管理生命周期：enabled / disabled / uninstalled；
// - 启停写 profile 受管 patch 区块（dsh live 热重载，无需重启）；
// - 装卸走官方 `dsh plugin --profile web ...` 通道（需要时自动重启 dsh）；
// - upstream 插件可一键同步；自研插件不参与同步。
import { useCallback, useEffect, useRef, useState } from "react";
import { useRefreshOnEvent } from "@/hooks/useTauriEvent";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import {
  listenPluginChanged,
  pluginInstall,
  pluginList,
  pluginRepair,
  pluginSetState,
  pluginSync,
  pluginUninstall,
  type PluginList,
  type PluginOrigin,
  type PluginView,
  type SyncReport,
} from "@/lib/tauri";
import { toast } from "sonner";
import { ChevronRight, Loader2 } from "lucide-react";

/** 状态徽章配色 */
function stateVariant(state: PluginView["state"]) {
  switch (state) {
    case "enabled":
      return "default" as const;
    case "disabled":
      return "secondary" as const;
    case "plain":
      return "outline" as const;
    default:
      return "destructive" as const;
  }
}

function stateLabel(state: PluginView["state"]) {
  switch (state) {
    case "enabled":
      return "已启用";
    case "disabled":
      return "已禁用";
    case "plain":
      return "普通依赖";
    default:
      return "未安装";
  }
}

function originLabel(origin: PluginOrigin) {
  switch (origin) {
    case "upstream":
      return "上游";
    case "in-house":
      return "自研";
    default:
      return "未知";
  }
}

export default function PluginsPanel() {
  const [data, setData] = useState<PluginList | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [spec, setSpec] = useState("");
  const [origin, setOrigin] = useState<PluginOrigin>("upstream");
  const [syncReport, setSyncReport] = useState<SyncReport | null>(null);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const list = await pluginList();
      if (mounted.current) setData(list);
    } catch (e) {
      toast.error(`读取插件列表失败: ${e}`);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    refresh();
    return () => {
      mounted.current = false;
    };
  }, [refresh]);

  // 订阅 plugin://changed（安装/启停/卸载/同步后由 Rust 广播）
  // 统一走 useTauriEvent（ADR-0009 D7）：此前用 `.then()` 无 `.catch` 且无竞态保护
  useRefreshOnEvent(listenPluginChanged, () => refresh(), [refresh]);

  /** 统一封装：占用 busy 标记 → 执行 → 刷新 → 提示 */
  async function run(key: string, action: () => Promise<string>) {
    setBusy(key);
    try {
      const message = await action();
      await refresh();
      if (message) toast.success(message);
    } catch (e) {
      toast.error(String(e));
    } finally {
      if (mounted.current) setBusy(null);
    }
  }

  async function toggle(item: PluginView, next: boolean) {
    const key = `toggle:${item.package}`;
    await run(key, async () => {
      const result = await pluginSetState(item.package, next);
      return result.status === "unchanged" ? "" : result.message;
    });
  }

  async function install() {
    const value = spec.trim();
    if (!value) {
      toast.info("请输入包名、git 地址或本地路径");
      return;
    }
    await run("install", async () => {
      const result = await pluginInstall(value, origin);
      setSpec("");
      return result.message;
    });
  }

  async function uninstall(item: PluginView) {
    if (item.protected) {
      toast.info(`${item.package} 是 profile 模板层，禁止卸载`);
      return;
    }
    await run(`uninstall:${item.package}`, async () => {
      const result = await pluginUninstall(item.package);
      return result.status === "unchanged" ? "" : result.message;
    });
  }

  async function sync(apply: boolean) {
    setSyncReport(null);
    await run(apply ? "sync" : "sync-check", async () => {
      const report = await pluginSync(apply);
      setSyncReport(report);
      const changed = report.items.filter((item) => item.result === "ok").length;
      const pending = report.items.filter((item) => item.result === "pending").length;
      const failed = report.items.filter((item) => item.result === "failed").length;
      if (!apply) {
        return pending > 0 ? `发现 ${pending} 个可更新插件` : "所有上游插件均为最新";
      }
      if (failed > 0) {
        toast.error(`${failed} 个插件同步失败，详见列表`);
      }
      return changed > 0 ? `已同步 ${changed} 个插件` : "无需要同步的插件";
    });
  }

  async function repair() {
    await run("repair", async () => {
      const result = await pluginRepair();
      return result.message;
    });
  }

  const degraded = data?.degradedReason ?? null;
  const plugins = data?.plugins ?? [];

  return (
    <div className="space-y-4">
      {/* 工具条：窄宽度下操作按钮组换行到统计行下方（三按钮不挤压标题） */}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="min-w-0 text-xs text-muted-foreground">
          profile <span className="font-medium text-foreground">{data?.profile ?? "web"}</span>
          <span className="mx-1.5 opacity-40">·</span>
          {plugins.length} 个插件
        </div>
        <div className="flex flex-wrap gap-1.5">
          <Button
            variant="outline"
            size="sm"
            disabled={busy !== null}
            onClick={() => sync(false)}
          >
            {busy === "sync-check" && <Loader2 className="size-3 animate-spin" />}
            检查更新
          </Button>
          <Button
            variant="secondary"
            size="sm"
            disabled={busy !== null}
            onClick={() => sync(true)}
          >
            {busy === "sync" && <Loader2 className="size-3 animate-spin" />}
            同步上游
          </Button>
          <Button
            variant="ghost"
            size="sm"
            disabled={busy !== null}
            onClick={repair}
            title="重新对账 bundles 并重放已保存的启停状态"
          >
            {busy === "repair" && <Loader2 className="size-3 animate-spin" />}
            收敛
          </Button>
        </div>
      </div>

      {degraded && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 p-2 text-xs text-destructive">
          {degraded}（启停已禁用，仅可查看）
        </div>
      )}

      {/* 安装 */}
      <div className="space-y-2">
        <Label htmlFor="plugin-spec" className="text-xs">
          安装插件（npm 包名 / GitHub URL / 本地路径）
        </Label>
        {/* 输入 + 来源 + 安装：窄宽度自动换行，输入框保持整行可读 */}
        <div className="flex flex-wrap items-center gap-1.5">
          <Input
            id="plugin-spec"
            value={spec}
            onChange={(e) => setSpec(e.target.value)}
            placeholder="dshmarket 或 https://github.com/owner/repo"
            className="min-w-[12rem] flex-1 text-xs"
          />
          <select
            className="dsh-select w-28 shrink-0"
            value={origin}
            onChange={(e) => setOrigin(e.target.value as PluginOrigin)}
            title="来源决定是否参与自动同步"
          >
            <option value="upstream">上游</option>
            <option value="in-house">自研</option>
            <option value="unknown">未知</option>
          </select>
          <Button
            variant="secondary"
            size="sm"
            className="shrink-0"
            disabled={busy !== null}
            onClick={install}
          >
            {busy === "install" && <Loader2 className="size-3 animate-spin" />}
            安装
          </Button>
        </div>
        <p className="text-[11px] leading-relaxed text-muted-foreground">
          <strong>npm 包名</strong>请填 registry 上的真实包名（例：
          <code className="dsh-code">dshmarket</code>，不是行 id
          <code className="dsh-code">dsh-market</code>）；
          <strong>GitHub 仓库</strong>可直接粘 URL
          （<code className="dsh-code">https://github.com/&lt;owner&gt;/&lt;repo&gt;</code>；
          建议钉 commit：<code className="dsh-code">…#&lt;sha&gt;</code>）；
          也可用 <code className="dsh-code">github:owner/repo</code>。
          pnpm 若拒绝执行构建脚本，请按安装日志的提示自行处理
          （启动器把 pnpm 输出原样转发，不代写任何 pnpm 配置）。
        </p>
      </div>

      <Separator />

      {/* 列表 */}
      <div className="space-y-2">
        {plugins.length === 0 && (
          <p className="text-xs text-muted-foreground">尚无受管插件。</p>
        )}
        {plugins.map((item) => {
          const canToggle =
            !degraded && (item.state === "enabled" || item.state === "disabled");
          const isOpen = expanded[item.package] === true;
          const isBusy = busy === `toggle:${item.package}` || busy === `uninstall:${item.package}`;
          return (
            <div key={item.package} className="dsh-list-row space-y-1.5 p-2">
              {/* 行头：窄宽度下控件组换行 */}
              <div className="flex flex-wrap items-center justify-between gap-x-2 gap-y-1.5">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-1.5">
                    <button
                      type="button"
                      className="flex min-w-0 items-center gap-1 truncate text-left text-xs font-medium hover:underline"
                      onClick={() =>
                        setExpanded((prev) => ({
                          ...prev,
                          [item.package]: !isOpen,
                        }))
                      }
                      title="展开行状态"
                      aria-expanded={isOpen}
                    >
                      <ChevronRight
                        className={`size-3 shrink-0 text-muted-foreground transition-transform duration-200 ${isOpen ? "rotate-90" : ""}`}
                      />
                      <span className="truncate">{item.package}</span>
                    </button>
                    {item.version && (
                      <span className="shrink-0 text-[11px] text-muted-foreground">
                        {item.version}
                      </span>
                    )}
                  </div>
                  <div className="mt-1 flex flex-wrap items-center gap-1">
                    <Badge variant={stateVariant(item.state)}>
                      {stateLabel(item.state)}
                    </Badge>
                    <Badge variant="outline">{originLabel(item.origin)}</Badge>
                    {item.needsReconcile && (
                      <Badge variant="destructive">待对账</Badge>
                    )}
                    {item.protected && <Badge variant="outline">受保护</Badge>}
                  </div>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <Switch
                    size="sm"
                    checked={item.state === "enabled"}
                    disabled={!canToggle || busy !== null}
                    onCheckedChange={(value) => toggle(item, value === true)}
                    title={
                      canToggle
                        ? "启用/禁用（dsh 热重载，无需重启）"
                        : "该状态不支持启停"
                    }
                  />
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={busy !== null || item.protected}
                    onClick={() => uninstall(item)}
                  >
                    {isBusy ? "处理中…" : "卸载"}
                  </Button>
                </div>
              </div>

              {item.lastError && (
                <div className="text-[11px] text-destructive">{item.lastError}</div>
              )}

              {isOpen && (
                <div className="space-y-1 border-t border-border/50 pt-1.5 text-[11px] text-muted-foreground">
                  <div>
                    spec:{" "}
                    <span className="text-foreground">
                      {item.installedSpec ?? "-"}
                    </span>
                  </div>
                  {item.source?.repo && (
                    <div>
                      repo:{" "}
                      <span className="text-foreground">{item.source.repo}</span>
                      {item.source.commit && (
                        <>
                          {" @ "}
                          <span className="text-foreground">
                            {item.source.commit.slice(0, 10)}
                          </span>
                        </>
                      )}
                    </div>
                  )}
                  {item.lastSync && (
                    <div>
                      上次同步: {item.lastSync.from || "-"} → {item.lastSync.to || "-"}（
                      {item.lastSync.result}）
                    </div>
                  )}
                  {item.rows.length > 0 && (
                    <div className="space-y-0.5">
                      {item.rows.map((row) => (
                        <div key={row.id} className="flex items-center gap-1.5">
                          <Badge
                            variant={
                              row.state === "enabled"
                                ? "default"
                                : row.state === "disabled"
                                  ? "secondary"
                                  : "outline"
                            }
                          >
                            {row.state === "enabled"
                              ? "启用"
                              : row.state === "disabled"
                                ? "禁用"
                                : "表达式"}
                          </Badge>
                          <span className="text-foreground">{row.id}</span>
                          {row.name && <span className="truncate">{row.name}</span>}
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {syncReport && syncReport.items.length > 0 && (
        <>
          <Separator />
          <div className="space-y-1 text-[11px]">
            <div className="text-xs font-medium">同步结果</div>
            {syncReport.items.map((item) => (
              <div key={item.package} className="flex flex-wrap items-start gap-1.5">
                <Badge
                  variant={
                    item.result === "ok"
                      ? "default"
                      : item.result === "failed"
                        ? "destructive"
                        : "outline"
                  }
                >
                  {item.result}
                </Badge>
                <span className="text-foreground">{item.package}</span>
                <span className="min-w-0 flex-1 text-muted-foreground">{item.message}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
