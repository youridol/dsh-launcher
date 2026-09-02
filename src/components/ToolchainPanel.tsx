// 工具链面板：检测 Node/npm/pnpm/Git/Python 状态 + 一键安装
// 实时更新：订阅 toolchain://changed（安装完成广播）自动刷新条目状态；
// 安装期间订阅 install://progress（channel=toolchain）显示进度条。
import { useCallback, useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CardDescription, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { Separator } from "@/components/ui/separator";
import {
  detectToolchain,
  installToolchain,
  listenToolchainChanged,
  type ToolchainItem,
} from "@/lib/tauri";
import { listenProgress, PHASE_LABELS, type InstallProgress } from "@/lib/install";
import { toast } from "sonner";
import { Wrench, RefreshCw } from "lucide-react";

const STATE_META: Record<string, { label: string; variant: "default" | "secondary" | "destructive" | "outline" }> = {
  present: { label: "已就绪", variant: "default" },
  missing: { label: "缺失", variant: "destructive" },
  mismatch: { label: "版本不符", variant: "secondary" },
};

// Git 为 UAC 异步安装（Rust 端 Start-Process 立即返回），需提示用户完成 UAC 后生效
const GIT_UAC_HINT = "Git 安装程序已启动：请在 UAC 弹窗中确认，完成后点击“重新检测”刷新状态";

export default function ToolchainPanel() {
  const [items, setItems] = useState<ToolchainItem[]>([]);
  // 正在安装的条目（null = 无安装任务）
  const [busyName, setBusyName] = useState<string | null>(null);
  // 安装进度（当前工具链安装任务；channel=toolchain 时显示）
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  // Git UAC 异步安装提示（安装启动后显示，直到用户手动刷新）
  const [gitUacPending, setGitUacPending] = useState(false);

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

  async function handleInstall(name: string) {
    setBusyName(name);
    setGitUacPending(false);
    setProgress({ channel: "toolchain", phase: "prepare", percent: 0, message: "准备中…" });
    try {
      const msg = await installToolchain(name);
      // Git 安装是 UAC 异步：Start-Process 立即返回，实际安装由用户在 UAC 中确认后完成
      // （Rust 端对 git 不广播 toolchain://changed，避免误报仍缺失）
      if (name === "git") {
        setGitUacPending(true);
        setBusyName(null);
        toast.success(msg, { description: GIT_UAC_HINT });
        return;
      }
      toast.success(msg);
      // 工具链变更事件会触发自动 refresh；这里兜底一次（事件可能先于本返回值到达）
      refresh();
    } catch (e) {
      setProgress(null);
      toast.error(`安装失败: ${e}`);
    } finally {
      // 同步安装（node/pnpm）完成后清除 busy 与进度条
      setBusyName(null);
      setProgress(null);
    }
  }

  return (
    <div className="flex flex-col">
      {/* 标题块（无卡片外壳：直接作为侧边栏面板内容） */}
      <div className="flex flex-col gap-1">
        <CardTitle className="flex items-center gap-2">
          <Wrench className="size-4" />
          工具链
          <Button variant="ghost" size="icon-sm" onClick={refresh} title="重新检测">
            <RefreshCw />
          </Button>
        </CardTitle>
        <CardDescription>dsh 运行依赖</CardDescription>
      </div>
      <Separator className="my-3" />
      <div className="space-y-2">
        {/* 安装进度条（channel=toolchain 时显示；done 后短暂保留 100%） */}
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
        {/* Git UAC 异步安装提示 */}
        {gitUacPending && (
          <p className="rounded-md border border-amber-500/30 bg-amber-500/10 px-2 py-1.5 text-xs text-amber-600">
            {GIT_UAC_HINT}
          </p>
        )}
        {items.map((item, i) => (
          <div key={item.name}>
            {i > 0 && <Separator className="mb-2" />}
            {/* 单行布局：标题 + 版本描述 + 安装按钮 一行排布（允许换行的场景保留） */}
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              {/* 标题与状态徽标 */}
              <span className="flex items-center gap-1.5 font-medium">
                <span className="truncate">{item.name}</span>
                <Badge variant={STATE_META[item.state]?.variant ?? "outline"}>
                  {STATE_META[item.state]?.label ?? item.state}
                </Badge>
              </span>
              {/* 版本描述：已安装版本 + 要求，与标题同行 */}
              <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                {item.installedVersion
                  ? `已安装 ${item.installedVersion}`
                  : "未安装"}
                <span className="ml-1.5">要求: {item.required}</span>
              </span>
              {/* 安装按钮（缺失/版本不符时显示，靠右） */}
              {item.state !== "present" && (
                <Button
                  variant="secondary"
                  size="sm"
                  className="shrink-0"
                  disabled={busyName !== null}
                  onClick={() => handleInstall(item.name)}
                >
                  {busyName === item.name ? "安装中…" : "一键安装"}
                </Button>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
