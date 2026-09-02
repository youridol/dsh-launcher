// 工具链面板：检测 Node/npm/pnpm/Git/Python 状态 + 一键安装
import { useCallback, useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CardDescription, CardTitle } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { detectToolchain, installToolchain, type ToolchainItem } from "@/lib/tauri";
import { toast } from "sonner";
import { Wrench, RefreshCw } from "lucide-react";

const STATE_META: Record<string, { label: string; variant: "default" | "secondary" | "destructive" | "outline" }> = {
  present: { label: "已就绪", variant: "default" },
  missing: { label: "缺失", variant: "destructive" },
  mismatch: { label: "版本不符", variant: "secondary" },
};

export default function ToolchainPanel() {
  const [items, setItems] = useState<ToolchainItem[]>([]);
  const [busyName, setBusyName] = useState<string | null>(null);

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

  async function handleInstall(name: string) {
    setBusyName(name);
    try {
      const msg = await installToolchain(name);
      toast.success(msg);
    } catch (e) {
      toast.error(`安装失败: ${e}`);
    } finally {
      setBusyName(null);
      refresh();
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
                  disabled={busyName === item.name}
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
