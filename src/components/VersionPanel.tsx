// 版本管理面板：双通道版本列表（按钮组切换 GitHub/npm）+ 安装/卸载（最新版本置顶）
// v0.1.7：安装过程显示进度条（订阅 install://progress 事件）
// v0.4.9：双通道列表融合——顶部按钮组切换 GitHub/npm，单一刷新按钮，默认 GitHub。
//   两通道列表在进入时并发预加载（refresh 一次性拉齐），切换 Tab 零网络开销、零状态丢失，
//   保证来回切换无 bug。安装任意通道版本时 Rust 端自动先卸载对侧通道（全局单版本）。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { Progress } from "@/components/ui/progress";
import { versionSortDesc } from "@/lib/version";
import {
  getInstalledVersion,
  getInstallPaths,
  installVersion,
  listVersions,
  listenVersionChanged,
  type DshVersion,
  type InstallPaths,
} from "@/lib/tauri";
import {
  listenProgress,
  PHASE_LABELS,
  type InstallProgress,
} from "@/lib/install";
import { toast } from "sonner";
import { Package, RefreshCw } from "lucide-react";
import ToolchainPanel from "@/components/ToolchainPanel";
import { cn } from "@/lib/utils";

/** 当前展示的通道 Tab（默认 GitHub） */
type ActiveChannel = "github" | "npm";

// 通道 Tab 元信息
const CHANNEL_TABS: { key: ActiveChannel; label: string; hint: string }[] = [
  { key: "github", label: "GitHub 通道", hint: "源码 clone + 构建" },
  { key: "npm", label: "npm 通道", hint: "registry 现成包" },
];

export default function VersionPanel() {
  const [npmVersions, setNpmVersions] = useState<DshVersion[]>([]);
  const [ghVersions, setGhVersions] = useState<DshVersion[]>([]);
  // 当前展示通道（默认 GitHub）
  const [active, setActive] = useState<ActiveChannel>("github");
  const [installed, setInstalled] = useState<string | null>(null);
  // 安装路径（显示 harness 下载/安装目录）
  const [paths, setPaths] = useState<InstallPaths | null>(null);
  // 刷新安装状态中
  const [refreshingInstalled, setRefreshingInstalled] = useState(false);
  const [busy, setBusy] = useState(false);
  // 正在刷新版本（单一刷新按钮；npm+github 并发拉取）
  const [refreshing, setRefreshing] = useState(false);
  // 安装进度（当前安装任务）
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  // 进度条延迟清理定时器句柄（v0.4.13，审计修复 L1：卸载时清理，避免卸载后 setState）
  const clearTimerRef = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (clearTimerRef.current !== null) {
        window.clearTimeout(clearTimerRef.current);
        clearTimerRef.current = null;
      }
    };
  }, []);

  // 最新版本置顶（npm 原始顺序是旧→新，需反转；GitHub 已是降序但统一处理）
  const sortedNpm = useMemo(
    () => [...npmVersions].sort((a, b) => versionSortDesc(a.version, b.version)),
    [npmVersions],
  );
  const sortedGh = useMemo(
    () => [...ghVersions].sort((a, b) => versionSortDesc(a.version, b.version)),
    [ghVersions],
  );

  const refresh = useCallback(async () => {
    // 并发拉取（互不阻塞）：npm 版本 + GitHub 版本 + 已安装版本 + 安装路径
    const [npm, gh, installed, paths] = await Promise.allSettled([
      listVersions("npm"),
      listVersions("github"),
      getInstalledVersion(),
      getInstallPaths(),
    ]);
    if (npm.status === "fulfilled") setNpmVersions(npm.value);
    else console.error("查询 npm 版本失败", npm.reason);
    if (gh.status === "fulfilled") setGhVersions(gh.value);
    else console.error("查询 GitHub 版本失败", gh.reason);
    if (installed.status === "fulfilled") setInstalled(installed.value);
    else setInstalled(null);
    if (paths.status === "fulfilled") setPaths(paths.value);
    else console.error("查询安装路径失败", paths.reason);
  }, []);

  // 单独刷新安装状态（不重新拉版本列表）
  const refreshInstalled = useCallback(async () => {
    setRefreshingInstalled(true);
    try {
      const [inst, paths] = await Promise.allSettled([
        getInstalledVersion(),
        getInstallPaths(),
      ]);
      if (inst.status === "fulfilled") setInstalled(inst.value);
      else setInstalled(null);
      if (paths.status === "fulfilled") setPaths(paths.value);
    } catch (e) {
      console.error("刷新安装状态失败", e);
      toast.error(`刷新安装状态失败: ${e}`);
    } finally {
      setRefreshingInstalled(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // 订阅 dsh 安装版本变更事件（卸载/安装后由 Rust 广播 → 刷新安装状态与路径）
  // 解决：卸载按钮在 StatusCard，而本面板 state 独立，卸载后不刷新（安装后因在本面板内才正常）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listenVersionChanged(() => {
          refreshInstalled();
        });
      } catch (e) {
        console.error("订阅版本变更事件失败", e);
      }
    })();
    return () => {
      unlisten?.();
    };
  }, [refreshInstalled]);

  // 单一刷新按钮：并发刷新两个通道版本列表（切换 Tab 只切展示，不触发网络）
  const refreshAllChannels = useCallback(async () => {
    setRefreshing(true);
    try {
      const [npm, gh] = await Promise.allSettled([
        listVersions("npm"),
        listVersions("github"),
      ]);
      if (npm.status === "fulfilled") setNpmVersions(npm.value);
      else {
        console.error("刷新 npm 版本失败", npm.reason);
        toast.error(`刷新 npm 版本列表失败: ${npm.reason}`);
      }
      if (gh.status === "fulfilled") setGhVersions(gh.value);
      else {
        console.error("刷新 GitHub 版本失败", gh.reason);
        toast.error(`刷新 GitHub 版本列表失败: ${gh.reason}`);
      }
    } finally {
      setRefreshing(false);
    }
  }, []);

  // 订阅安装进度事件（Rust 端 install://progress）
  // 仅处理 dsh 版本安装通道（npm/github）；工具链安装进度由 ToolchainPanel 自己订阅，
  // 避免互相污染进度条
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listenProgress((p) => {
          if (p.channel === "npm" || p.channel === "github") {
            setProgress(p);
          }
        });
      } catch (e) {
        console.error("订阅安装进度事件失败", e);
      }
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  async function handleInstall(channel: string, version: string) {
    setBusy(true);
    setProgress({ channel, phase: "prepare", percent: 0, message: "准备中…" });
    try {
      const msg = await installVersion(channel, version);
      setProgress({ channel, phase: "done", percent: 100, message: "安装完成" });
      toast.success(msg);
      // 安装完成后自动切到该通道 Tab（让用户看到新装的“当前版本”）
      if (channel === "github" || channel === "npm") setActive(channel);
      refresh();
    } catch (e) {
      setProgress(null);
      toast.error(`安装失败: ${e}`);
    } finally {
      setBusy(false);
      // 完成后延迟清除进度条（让用户看到 100%）——句柄登记，卸载时清理（审计修复 L1）
      if (clearTimerRef.current !== null) window.clearTimeout(clearTimerRef.current);
      clearTimerRef.current = window.setTimeout(
        () => setProgress((p) => (p?.phase === "done" ? null : p)),
        2500,
      );
    }
  }

  const channelBadge = (c: string) => (
    <Badge variant={c === "npm" ? "secondary" : "outline"}>{c}</Badge>
  );

  // 安装状态显示：github:x.y.z / npm:x.y.z → 去掉前缀
  const installedLabel = (() => {
    if (!installed) return null;
    const idx = installed.indexOf(":");
    if (idx === -1) return installed;
    const ch = installed.slice(0, idx);
    const ver = installed.slice(idx + 1);
    return `${ch === "github" ? "GitHub" : "npm"} ${ver}`;
  })();

  // 判断某通道某版本是否为当前安装版本（installed 前缀 + 版本匹配）
  const isCurrent = (channel: string, version: string) => {
    if (!installed) return false;
    const idx = installed.indexOf(":");
    const ch = idx === -1 ? "" : installed.slice(0, idx);
    const ver = idx === -1 ? installed : installed.slice(idx + 1);
    // GitHub 列表 tag 形如 dsh-v0.1.2-alpha.1，package.json version 为 0.1.2-alpha.1
    const ver2 = version.replace(/^dsh-v/, "");
    return ch === channel && (ver === version || ver === ver2);
  };

  // 当前展示通道的版本列表
  const activeVersions = active === "github" ? sortedGh : sortedNpm;
  const activeEmpty = active === "github" ? ghVersions.length === 0 : npmVersions.length === 0;

  return (
    <Card className="flex min-h-0 flex-1 flex-col">
      <CardHeader className="shrink-0">
        <CardTitle className="flex items-center gap-2">
          <Package className="size-4" />
          版本管理
          <Badge variant="default">{installedLabel ?? "未安装"}</Badge>
          <div className="ml-auto flex items-center gap-1.5">
            <Button
              variant="ghost"
              size="icon-sm"
              disabled={refreshingInstalled}
              onClick={refreshInstalled}
              title="刷新安装状态"
            >
              <RefreshCw className={refreshingInstalled ? "animate-spin" : ""} />
            </Button>
          </div>
        </CardTitle>
        <CardDescription>GitHub / npm 双通道 · 全局单版本（切换安装自动卸载对侧）</CardDescription>
      </CardHeader>
      <CardContent className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto">
        {/* 通道切换按钮组 + 单一刷新按钮 */}
        <div className="flex shrink-0 flex-wrap items-center gap-2">
          <div
            className="inline-flex items-center gap-0.5 rounded-lg bg-muted p-0.5"
            role="tablist"
            aria-label="安装通道切换"
          >
            {CHANNEL_TABS.map((t) => {
              const isActive = active === t.key;
              return (
                <Button
                  key={t.key}
                  size="sm"
                  variant={isActive ? "default" : "ghost"}
                  role="tab"
                  aria-selected={isActive}
                  onClick={() => setActive(t.key)}
                  title={t.hint}
                  className={cn(
                    "px-3",
                    !isActive && "text-muted-foreground hover:bg-muted/70",
                  )}
                >
                  {t.label}
                </Button>
              );
            })}
          </div>
          <Button
            variant="outline"
            size="sm"
            disabled={refreshing}
            onClick={refreshAllChannels}
            title="刷新全部通道版本列表"
          >
            <RefreshCw className={refreshing ? "animate-spin" : "size-3"} />
            刷新列表
          </Button>
        </div>

        {/* 安装/下载目录（任务：显示 harness 下载和安装目录） */}
        {paths && (
          <div className="space-y-1 rounded-md border bg-muted/30 p-3 text-xs">
            <div className="flex items-center justify-between">
              <span className="font-medium">GitHub 安装目录</span>
              <Badge variant={paths.githubInstalled ? "default" : "outline"}>
                {paths.githubInstalled ? "已安装" : "未安装"}
              </Badge>
            </div>
            <code className="block break-all text-muted-foreground">{paths.githubDir}</code>
            <Separator className="my-2" />
            <span className="font-medium">npm 全局目录</span>
            <code className="block break-all text-muted-foreground">{paths.npmGlobalDir}</code>
            <code className="block break-all text-muted-foreground">↳ bin: {paths.npmBinDir}</code>
          </div>
        )}
        {/* 安装进度条（busy 或 done 时显示） */}
        {progress && (
          <div className="rounded-md border bg-muted/30 p-3">
            <div className="mb-2 flex items-center justify-between text-xs">
              <span className="font-medium">
                {PHASE_LABELS[progress.phase] ?? progress.phase}
              </span>
              <span className="tabular-nums text-muted-foreground">
                {progress.percent}%
              </span>
            </div>
            {/* 进度条：value 驱动 indicator 宽度 */}
            <Progress value={progress.percent} />
            <p className="mt-2 truncate text-xs text-muted-foreground">
              {progress.message}
            </p>
          </div>
        )}
        {/* 单一融合版本列表（按钮组切换 GitHub/npm） */}
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="mb-2 flex shrink-0 items-center justify-between">
            <h3 className="text-sm font-medium">
              {active === "github" ? "GitHub 通道版本" : "npm 通道版本"}
              <span className="ml-2 text-xs text-muted-foreground">
                {active === "github"
                  ? "源码 clone + pnpm 构建"
                  : "npm registry 现成包"}
                {installed && active === installed.split(":")[0] ? " · 当前使用" : ""}
              </span>
            </h3>
          </div>
          <div className="min-h-0 flex-1 space-y-1 overflow-y-auto">
            {/* 最新 + 7 个历史版本 */}
            {activeVersions.slice(0, 8).map((v) => {
              const isCur = isCurrent(v.channel, v.version);
              return (
                <div
                  key={`${v.channel}-${v.version}`}
                  className="flex items-center justify-between rounded px-2 py-1 hover:bg-muted"
                >
                  <span className="flex min-w-0 items-center gap-2 text-sm">
                    <span className="truncate">{v.version}</span>
                    {channelBadge(v.channel)}
                  </span>
                  <Button
                    size="xs"
                    variant="secondary"
                    disabled={busy || isCur}
                    onClick={() => handleInstall(v.channel, v.version)}
                    title={
                      isCur
                        ? "当前版本"
                        : v.channel !== active
                          ? `切换到 ${v.channel} 通道（自动卸载当前通道）`
                          : "安装此版本"
                    }
                  >
                    {isCur ? "当前版本" : v.channel !== active ? "切换安装" : "安装"}
                  </Button>
                </div>
              );
            })}
            {activeEmpty && (
              <div className="text-xs text-muted-foreground">
                {active === "github"
                  ? "暂无版本（请检查 GitHub 网络/镜像，或点击“刷新列表”）"
                  : "暂无版本（请检查 npm 网络/镜像，或点击“刷新列表”）"}
              </div>
            )}
          </div>
        </div>
        {/* 分割线 + 工具链（从左侧边栏移入：位于版本列表底部） */}
        <Separator className="my-1" />
        <div className="shrink-0">
          <ToolchainPanel />
        </div>
      </CardContent>
    </Card>
  );
}
