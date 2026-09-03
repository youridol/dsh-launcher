// 工具链面板：检测 Node/npm/pnpm/Git/Python 状态 + 安装/卸载/批量操作
// 实时更新：订阅 toolchain://changed（安装完成广播）自动刷新条目状态；
// 安装期间订阅 install://progress（channel=toolchain）显示进度条。
// v0.4.0：
// - 每条工具链支持 卸载（Node 删用户级目录 + 清 PATH；Git/Python 走官方卸载器 UAC）
// - 顶部工具条：批量一键安装缺失 / 一键卸载全部（带确认对话框）
// - 支持 python 安装（Rust 端官方安装包静默安装）
import { useCallback, useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CardDescription, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { Separator } from "@/components/ui/separator";
import {
  batchInstallToolchains,
  batchUninstallToolchains,
  detectToolchain,
  installToolchain,
  listenToolchainChanged,
  uninstallToolchain,
  type BatchResult,
  type ToolchainItem,
} from "@/lib/tauri";
import { listenProgress, PHASE_LABELS, type InstallProgress } from "@/lib/install";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { toast } from "sonner";
import { Wrench, RefreshCw, PackageCheck, Trash2, Download } from "lucide-react";

const STATE_META: Record<string, { label: string; variant: "default" | "secondary" | "destructive" | "outline" }> = {
  present: { label: "已就绪", variant: "default" },
  missing: { label: "缺失", variant: "destructive" },
  mismatch: { label: "版本不符", variant: "secondary" },
};

// 可卸载/可安装项元信息
const ITEM_LABEL: Record<string, string> = {
  node: "Node.js",
  npm: "npm",
  pnpm: "pnpm",
  git: "Git",
  python: "Python",
};

// Git/Python 为 UAC 异步安装/卸载（Rust 端 Start-Process 立即返回），需提示用户完成 UAC 后生效
const UAC_HINT = "安装程序已启动：请在 UAC 弹窗中确认，完成后点击“重新检测”刷新状态";

export default function ToolchainPanel() {
  const [items, setItems] = useState<ToolchainItem[]>([]);
  // 正在操作的条目（null = 无任务）
  const [busyName, setBusyName] = useState<string | null>(null);
  // 批量操作进行中
  const [batchBusy, setBatchBusy] = useState(false);
  // 安装进度（当前工具链安装任务；channel=toolchain 时显示）
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  // Git/Python UAC 异步提示（安装启动后显示，直到手动刷新）
  const [uacPending, setUacPending] = useState(false);
  // 一键卸载确认弹窗
  const [confirmUninstall, setConfirmUninstall] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setItems(await detectToolchain());
    } catch (e) {
      console.error("检测工具链失败", e);
      toast.error(`检测工具链失败: ${e}`);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // 订阅工具链变更事件（Rust 在 install_toolchain 成功后广播 → 自动刷新条目状态）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listenToolchainChanged(() => {
          refresh();
        });
      } catch (e) {
        console.error("订阅工具链变更事件失败", e);
      }
    })();
    return () => {
      unlisten?.();
    };
  }, [refresh]);

  // 订阅安装进度事件（仅处理 channel=toolchain，避免污染其他面板的进度显示）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listenProgress((p) => {
          if (p.channel === "toolchain") {
            setProgress(p);
            // 安装完成阶段自动结束 busy 态
            if (p.phase === "done") setBusyName(null);
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

  // 汇总批量结果 toast（成功/失败分开提示）
  function showBatchResults(results: BatchResult[], action: string) {
    const ok = results.filter((r) => r.ok);
    const fail = results.filter((r) => !r.ok);
    if (ok.length > 0) {
      toast.success(`${action}完成：${ok.map((r) => ITEM_LABEL[r.name] ?? r.name).join("、")}`);
    }
    fail.forEach((r) => {
      toast.error(`${ITEM_LABEL[r.name] ?? r.name} ${action}失败: ${r.message}`);
    });
  }

  // 异步 UAC（git/python）触发的提示逻辑
  function markUacIfNeeded(name: string, msg: string) {
    if (name === "git" || name === "python") {
      setUacPending(true);
      setBusyName(null);
      toast.success(msg, { description: UAC_HINT });
    } else {
      toast.success(msg);
    }
  }

  // 单项安装
  async function handleInstall(name: string) {
    setBusyName(name);
    setUacPending(false);
    setProgress({ channel: "toolchain", phase: "prepare", percent: 0, message: "准备中…" });
    try {
      const msg = await installToolchain(name);
      markUacIfNeeded(name, msg);
      // 同步安装（node/pnpm）完成后：变更事件触发 refresh；这里兜底一次
      if (name === "node" || name === "pnpm") {
        refresh();
      }
    } catch (e) {
      setProgress(null);
      toast.error(`安装失败: ${e}`);
    } finally {
      if (name === "node" || name === "pnpm") {
        setBusyName(null);
        setProgress(null);
      } else {
        // git/python 为 UAC 异步：完成后清进度条（不再显示停留的 100%）
        setProgress(null);
      }
    }
  }

  // 单项卸载（Node 同步；Git/Python UAC 异步）
  async function handleUninstall(name: string) {
    setBusyName(name);
    setUacPending(false);
    try {
      const msg = await uninstallToolchain(name);
      markUacIfNeeded(name, msg);
      refresh();
    } catch (e) {
      toast.error(`卸载失败: ${e}`);
    } finally {
      if (name === "node") {
        setBusyName(null);
      }
    }
  }

  // 批量一键安装缺失
  async function handleBatchInstall() {
    setBatchBusy(true);
    setUacPending(false);
    setProgress({ channel: "toolchain", phase: "prepare", percent: 0, message: "批量安装中…" });
    try {
      const results = await batchInstallToolchains();
      showBatchResults(results, "批量安装");
      refresh();
    } catch (e) {
      toast.error(`批量安装失败: ${e}`);
    } finally {
      setBatchBusy(false);
      setProgress(null);
      setBusyName(null);
    }
  }

  // 一键卸载全部（确认后）
  async function handleBatchUninstall() {
    setConfirmUninstall(false);
    setBatchBusy(true);
    setUacPending(false);
    try {
      const results = await batchUninstallToolchains();
      showBatchResults(results, "批量卸载");
      refresh();
    } catch (e) {
      toast.error(`批量卸载失败: ${e}`);
    } finally {
      setBatchBusy(false);
      setBusyName(null);
    }
  }

  return (
    <div className="flex flex-col">
      {/* 标题块 */}
      <div className="flex flex-col gap-1">
        <CardTitle className="flex items-center gap-2">
          <Wrench className="size-4" />
          工具链
          <Button variant="ghost" size="icon-sm" onClick={refresh} title="重新检测">
            <RefreshCw />
          </Button>
          {/* 批量操作 */}
          <div className="ml-auto flex gap-1.5">
            <Button
              variant="outline"
              size="xs"
              disabled={batchBusy || busyName !== null}
              onClick={handleBatchInstall}
              title="一键安装全部缺失工具链（Node→pnpm→Git→Python）"
            >
              <PackageCheck className="size-3.5" /> 一键安装缺失
            </Button>
            <Button
              variant="destructive"
              size="xs"
              disabled={batchBusy || busyName !== null}
              onClick={() => setConfirmUninstall(true)}
              title="卸载全部工具链（Node/Git/Python；npm/pnpm 随 Node 目录移除）"
            >
              <Trash2 className="size-3.5" /> 一键卸载
            </Button>
          </div>
        </CardTitle>
        <CardDescription>dsh 运行依赖 · 安装/卸载/批量管理</CardDescription>
      </div>
      <Separator className="my-3" />
      <div className="space-y-2">
        {/* 安装进度条（channel=toolchain 时显示） */}
        {progress && (
          <div className="rounded-md border bg-muted/30 p-2">
            <div className="mb-1 flex items-center justify-between text-xs">
              <span className="font-medium">{PHASE_LABELS[progress.phase] ?? progress.phase}</span>
              <span className="tabular-nums text-muted-foreground">{progress.percent}%</span>
            </div>
            <Progress value={progress.percent} />
            <p className="mt-1 truncate text-xs text-muted-foreground">{progress.message}</p>
          </div>
        )}
        {/* Git/Python UAC 异步提示 */}
        {uacPending && (
          <p className="rounded-md border border-amber-500/30 bg-amber-500/10 px-2 py-1.5 text-xs text-amber-600">
            {UAC_HINT}
          </p>
        )}
        {items.map((item, i) => (
          <div key={item.name}>
            {i > 0 && <Separator className="mb-2" />}
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <span className="flex items-center gap-1.5 font-medium">
                <span className="truncate">{ITEM_LABEL[item.name] ?? item.name}</span>
                <Badge variant={STATE_META[item.state]?.variant ?? "outline"}>
                  {STATE_META[item.state]?.label ?? item.state}
                </Badge>
              </span>
              <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                {item.installedVersion
                  ? `已安装 ${item.installedVersion}`
                  : "未安装"}
                <span className="ml-1.5">要求: {item.required}</span>
              </span>
              {/* 已安装 → 卸载按钮（npm/pnpm 不单独卸载，随 Node 目录删除） */}
              {item.state === "present" &&
                (item.name === "node" || item.name === "git" || item.name === "python") && (
                  <Button
                    variant="outline"
                    size="xs"
                    className="shrink-0"
                    disabled={busyName !== null || batchBusy}
                    onClick={() => handleUninstall(item.name)}
                    title={`卸载 ${ITEM_LABEL[item.name] ?? item.name}`}
                  >
                    <Trash2 className="size-3" />
                    {busyName === item.name ? "卸载中…" : "卸载"}
                  </Button>
                )}
              {/* 未就绪 → 安装按钮 */}
              {item.state !== "present" && item.name !== "npm" && (
                <Button
                  variant="secondary"
                  size="xs"
                  className="shrink-0"
                  disabled={busyName !== null || batchBusy}
                  onClick={() => handleInstall(item.name)}
                >
                  <Download className="size-3" />
                  {busyName === item.name ? "安装中…" : "一键安装"}
                </Button>
              )}
              {/* npm 特殊提示：不可独立安装（随 Node） */}
              {item.state !== "present" && item.name === "npm" && (
                <span className="shrink-0 text-[11px] text-muted-foreground">
                  随 Node 提供
                </span>
              )}
            </div>
          </div>
        ))}
      </div>

      {/* 一键卸载全部确认对话框 */}
      <Dialog open={confirmUninstall} onOpenChange={setConfirmUninstall}>
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>一键卸载全部工具链？</DialogTitle>
            <DialogDescription>
              将卸载：Node.js（含 npm/pnpm 用户级安装）、Git、Python。
              <br />
              Git/Python 会启动官方卸载器（需 UAC 确认）；卸载后 dsh 源码构建将不可用，
              如需继续使用请重新安装。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" size="sm" onClick={() => setConfirmUninstall(false)}>
              取消
            </Button>
            <Button
              variant="destructive"
              size="sm"
              disabled={batchBusy}
              onClick={handleBatchUninstall}
            >
              确认卸载
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
