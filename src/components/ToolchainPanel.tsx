// 工具链面板：检测 Node/npm/pnpm/Git/Python 状态 + 一键安装
import { useCallback, useEffect, useState } from "react";
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
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Wrench className="size-4" />
          工具链与运行环境
          <Button variant="ghost" size="icon-sm" onClick={refresh} title="重新检测">
            <RefreshCw />
          </Button>
        </CardTitle>
        <CardDescription>dsh 运行依赖（Node/npm/pnpm/Git/Python）</CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {items.map((item, i) => (
          <div key={item.name}>
            {i > 0 && <Separator className="mb-3" />}
            <div className="flex items-center justify-between gap-2">
              <div>
                <div className="flex items-center gap-2">
                  <span className="font-medium">{item.name}</span>
                  <Badge variant={STATE_META[item.state]?.variant ?? "outline"}>
                    {STATE_META[item.state]?.label ?? item.state}
                  </Badge>
                </div>
                <div className="text-xs text-muted-foreground">
                  {item.installedVersion
                    ? `已安装 ${item.installedVersion}`
                    : "未安装"}
                  <span className="ml-2">要求: {item.required}</span>
                </div>
              </div>
              {item.state !== "present" && (
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={busyName === item.name}
                  onClick={() => handleInstall(item.name)}
                >
                  {busyName === item.name ? "安装中…" : "一键安装"}
                </Button>
              )}
            </div>
          </div>
        ))}
      </CardContent>
    </Card>
  );
}
