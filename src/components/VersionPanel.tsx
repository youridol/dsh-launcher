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
  installVersion,
  listVersions,
  uninstallDsh,
  type DshVersion,
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
    // 并发拉取（互不阻塞）：npm 版本 + GitHub 版本 + 已安装版本
    const [npm, gh, installed] = await Promise.allSettled([
      listVersions("npm"),
      listVersions("github"),
      getInstalledVersion(),
    ]);
    if (npm.status === "fulfilled") setNpmVersions(npm.value);
    else console.error("查询 npm 版本失败", npm.reason);
    if (gh.status === "fulfilled") setGhVersions(gh.value);
    else console.error("查询 GitHub 版本失败", gh.reason);
    if (installed.status === "fulfilled") setInstalled(installed.value);
    else setInstalled(null);
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

  async function handleUninstall() {
    setBusy(true);
    try {
      const msg = await uninstallDsh();
      toast.success(msg);
      refresh();
    } catch (e) {
      toast.error(`卸载失败: ${e}`);
    } finally {
      setBusy(false);
    }
  }

  const channelBadge = (c: string) => (
    <Badge variant={c === "npm" ? "secondary" : "outline"}>{c}</Badge>
  );

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Package className="size-4" />
          版本管理
          <Badge variant="default">{installed ? `已安装 ${installed}` : "未安装"}</Badge>
        </CardTitle>
        <CardDescription>npm 通道 / GitHub 通道（全局单版本，切换先卸载再装）</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
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
        <div>
          <div className="mb-2 flex items-center justify-between">
            <h3 className="text-sm font-medium">npm 通道（registry 现有版本，最新置顶）</h3>
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
          <div className="max-h-64 space-y-1 overflow-y-auto">
            {sortedNpm.map((v) => (
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
                  disabled={busy || installed === v.version}
                  onClick={() => handleInstall("npm", v.version)}
                >
                  {installed === v.version ? "当前版本" : "安装"}
                </Button>
              </div>
            ))}
            {npmVersions.length === 0 && (
              <div className="text-xs text-muted-foreground">暂无版本（请检查 npm 网络/镜像）</div>
            )}
          </div>
        </div>

        <Separator />

        <div>
          <div className="mb-2 flex items-center justify-between">
            <h3 className="text-sm font-medium">GitHub 通道（v0.1.2-alpha.1 及更新源码，最新置顶）</h3>
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
          <div className="max-h-64 space-y-1 overflow-y-auto">
            {sortedGh.map((v) => (
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
                  disabled={busy || installed === v.version}
                  onClick={() => handleInstall("github", v.version)}
                >
                  {installed === v.version ? "当前版本" : "安装"}
                </Button>
              </div>
            ))}
            {ghVersions.length === 0 && (
              <div className="text-xs text-muted-foreground">暂无版本（请检查 GitHub 网络/镜像）</div>
            )}
          </div>
        </div>

        <Separator />

        <div className="flex items-center gap-2">
          <Button variant="destructive" disabled={busy || !installed} onClick={handleUninstall}>
            卸载 dsh
          </Button>
          <span className="text-xs text-muted-foreground">
            卸载后 dsh 代码被移除；DSH_HOME 用户数据默认保留
          </span>
        </div>
      </CardContent>
    </Card>
  );
}
