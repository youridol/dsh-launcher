// 版本管理面板：双通道版本列表 + 安装/卸载（最新版本置顶）
// v0.1.7：安装过程显示进度条（订阅 install://progress 事件）
import { useCallback, useEffect, useMemo, useState } from "react";
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

export default function VersionPanel() {
  const [npmVersions, setNpmVersions] = useState<DshVersion[]>([]);
  const [ghVersions, setGhVersions] = useState<DshVersion[]>([]);
  const [installed, setInstalled] = useState<string | null>(null);
  // 安装路径（显示 harness 下载/安装目录）
  const [paths, setPaths] = useState<InstallPaths | null>(null);
  // 刷新安装状态中
  const [refreshingInstalled, setRefreshingInstalled] = useState(false);
  const [busy, setBusy] = useState(false);
  // 正在刷新版本的通道（npm / github / null）
  const [refreshing, setRefreshing] = useState<"npm" | "github" | null>(null);
  // 安装进度（当前安装任务）
  const [progress, setProgress] = useState<InstallProgress | null>(null);

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

  // 单独刷新某通道版本列表（npm / github 独立按钮）
  const refreshChannel = useCallback(async (channel: "npm" | "github") => {
    setRefreshing(channel);
    try {
      const versions = await listVersions(channel);
      if (channel === "npm") setNpmVersions(versions);
      else setGhVersions(versions);
    } catch (e) {
      console.error(`刷新 ${channel} 版本失败`, e);
      toast.error(`刷新 ${channel} 版本列表失败: ${e}`);
    } finally {
      setRefreshing(null);
    }
  }, []);

  // 订阅安装进度事件（Rust 端 install://progress）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listenProgress((p) => {
          setProgress(p);
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
      refresh();
    } catch (e) {
      setProgress(null);
      toast.error(`安装失败: ${e}`);
    } finally {
      setBusy(false);
      // 完成后延迟清除进度条（让用户看到 100%）
      setTimeout(() => setProgress((p) => (p?.phase === "done" ? null : p)), 2500);
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

  return (
    <Card className="flex min-h-0 flex-1 flex-col">
      <CardHeader className="shrink-0">
        <CardTitle className="flex items-center gap-2">
          <Package className="size-4" />
          版本管理
          <Badge variant="default">{installedLabel ?? "未安装"}</Badge>
          <Button
            variant="ghost"
            size="icon-sm"
            disabled={refreshingInstalled}
            onClick={refreshInstalled}
            title="刷新安装状态"
          >
            <RefreshCw className={refreshingInstalled ? "animate-spin" : ""} />
          </Button>
        </CardTitle>
        <CardDescription>npm / GitHub 双通道安装</CardDescription>
      </CardHeader>
      <CardContent className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto">
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
        {/* 双通道左右排列（大屏两列；每通道展示最新 + 7 个历史版本；列内列表自适应高度滚动） */}
        <div className="grid min-h-0 flex-1 grid-cols-1 gap-4 sm:grid-cols-2">
        <div className="flex min-h-0 flex-col">
          <div className="mb-2 flex shrink-0 items-center justify-between">
            <h3 className="text-sm font-medium">npm 通道</h3>
            <Button
              variant="ghost"
              size="icon-sm"
              disabled={refreshing !== null}
              onClick={() => refreshChannel("npm")}
              title="刷新 npm 版本列表"
            >
              <RefreshCw className={refreshing === "npm" ? "animate-spin" : ""} />
            </Button>
          </div>
          <div className="min-h-0 flex-1 space-y-1 overflow-y-auto">
            {/* 最新 + 7 个历史版本 */}
            {sortedNpm.slice(0, 8).map((v) => (
              <div
                key={v.version}
                className="flex items-center justify-between rounded px-2 py-1 hover:bg-muted"
              >
                <span className="text-sm">
                  {v.version} {channelBadge(v.channel)}
                </span>
                <Button
                  size="xs"
                  variant="secondary"
                  disabled={busy || isCurrent("npm", v.version)}
                  onClick={() => handleInstall("npm", v.version)}
                >
                  {isCurrent("npm", v.version) ? "当前版本" : "安装"}
                </Button>
              </div>
            ))}
            {npmVersions.length === 0 && (
              <div className="text-xs text-muted-foreground">暂无版本（请检查 npm 网络/镜像）</div>
            )}
          </div>
        </div>

        <div className="flex min-h-0 flex-col">
          <div className="mb-2 flex shrink-0 items-center justify-between">
            <h3 className="text-sm font-medium">GitHub 通道</h3>
            <Button
              variant="ghost"
              size="icon-sm"
              disabled={refreshing !== null}
              onClick={() => refreshChannel("github")}
              title="刷新 GitHub 版本列表"
            >
              <RefreshCw className={refreshing === "github" ? "animate-spin" : ""} />
            </Button>
          </div>
          <div className="min-h-0 flex-1 space-y-1 overflow-y-auto">
            {/* 最新 + 7 个历史版本 */}
            {sortedGh.slice(0, 8).map((v) => (
              <div
                key={v.version}
                className="flex items-center justify-between rounded px-2 py-1 hover:bg-muted"
              >
                <span className="text-sm">
                  {v.version} {channelBadge(v.channel)}
                </span>
                <Button
                  size="xs"
                  variant="secondary"
                  disabled={busy || isCurrent("github", v.version)}
                  onClick={() => handleInstall("github", v.version)}
                >
                  {isCurrent("github", v.version) ? "当前版本" : "安装"}
                </Button>
              </div>
            ))}
            {ghVersions.length === 0 && (
              <div className="text-xs text-muted-foreground">暂无版本（请检查 GitHub 网络/镜像）</div>
            )}
          </div>
        </div>
        </div>
      </CardContent>
    </Card>
  );
}
