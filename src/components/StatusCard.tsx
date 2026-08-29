// 状态卡片：dsh 运行状态 + 生命周期控制 + 端口配置 + Web GUI（整合）
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  getDshStatus,
  getConfig,
  restartDsh,
  setPort as savePort,
  startDsh,
  stopDsh,
  type DshStatus as Status,
} from "@/lib/tauri";
import { toast } from "sonner";
import { Play, Square, RotateCw, Activity, ExternalLink, MonitorUp } from "lucide-react";

// 状态徽标配色
const STATUS_META: Record<Status, { label: string; variant: "default" | "secondary" | "destructive" | "outline" }> = {
  stopped: { label: "未运行", variant: "outline" },
  starting: { label: "启动中", variant: "secondary" },
  running: { label: "运行中", variant: "default" },
  stopping: { label: "停止中", variant: "secondary" },
  error: { label: "出错", variant: "destructive" },
};

export default function StatusCard() {
  const [status, setStatus] = useState<Status>("stopped");
  const [port, setPort] = useState("3080");
  const [busy, setBusy] = useState(false);

  // 拉取状态
  const refresh = useCallback(async () => {
    try {
      const s = await getDshStatus();
      setStatus(s);
    } catch (e) {
      console.error("获取状态失败", e);
    }
  }, []);

  useEffect(() => {
    refresh();
    // 读取配置端口（旧配置可能存了 0，兜底为 3080）
    getConfig()
      .then((c) => {
        const p = c.port;
        if (p >= 1 && p <= 65535) {
          setPort(p.toString());
        } else {
          // 端口非法（0/默认）→ 重置为默认 3080 并回写
          setPort("3080");
          savePort(3080).catch(() => {});
        }
      })
      .catch((e) => console.error("读取配置失败", e));
    // 5 秒轮询兜底（Q27）
    const timer = setInterval(refresh, 5000);
    return () => clearInterval(timer);
  }, [refresh]);

  async function handleStart() {
    setBusy(true);
    try {
      // 先保存端口再启动
      const p = Number(port);
      if (!Number.isInteger(p) || p < 1 || p > 65535) {
        toast.error("端口不合法（1-65535）");
        return;
      }
      await savePort(p);
      await startDsh();
      toast.success("dsh 已启动");
    } catch (e) {
      toast.error(`启动失败: ${e}`);
    } finally {
      setBusy(false);
      refresh();
    }
  }

  async function handleStop() {
    setBusy(true);
    try {
      await stopDsh();
      toast.success("dsh 已停止");
    } catch (e) {
      toast.error(`停止失败: ${e}`);
    } finally {
      setBusy(false);
      refresh();
    }
  }

  async function handleRestart() {
    setBusy(true);
    try {
      await restartDsh();
      toast.success("dsh 已重启");
    } catch (e) {
      toast.error(`重启失败: ${e}`);
    } finally {
      setBusy(false);
      refresh();
    }
  }

  const running = status === "running" || status === "starting";

  // Web GUI：内嵌 Tauri WebviewWindow / 外部浏览器
  async function openWebGui() {
    const label = "dsh-web-gui";
    const existing = await WebviewWindow.getByLabel(label);
    if (existing) {
      await existing.setFocus();
      return;
    }
    try {
      await new WebviewWindow(label, {
        url: `http://127.0.0.1:${port}`,
        title: "deepseek-harness Web UI",
        width: 1100,
        height: 750,
      });
    } catch (e) {
      console.error("打开 Web GUI 失败", e);
      toast.error(`打开 Web GUI 失败: ${e}`);
    }
  }

  function openExternal() {
    window.open(`http://127.0.0.1:${port}`, "_blank");
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Activity className="size-4" />
          dsh 运行状态
          <Badge variant={STATUS_META[status].variant}>{STATUS_META[status].label}</Badge>
        </CardTitle>
        <CardDescription>dsh 生命周期与 Web GUI</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex items-end gap-4">
          <div className="grid gap-1.5">
            <Label htmlFor="port">启动端口</Label>
            <Input
              id="port"
              type="number"
              min={1}
              max={65535}
              value={port}
              onChange={(e) => setPort(e.target.value)}
              className="w-32"
              disabled={running}
            />
          </div>
          <div className="flex gap-2">
            <Button onClick={handleStart} disabled={running || busy}>
              <Play /> 启动
            </Button>
            <Button variant="secondary" onClick={handleStop} disabled={!running || busy}>
              <Square /> 停止
            </Button>
            <Button variant="outline" onClick={handleRestart} disabled={!running || busy}>
              <RotateCw /> 重启
            </Button>
          </div>
        </div>

        <Separator className="my-4" />

        {/* Web GUI（原独立面板，整合至此） */}
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <MonitorUp className="size-4 text-muted-foreground" />
            <span className="text-sm font-medium">Web GUI</span>
            <Badge variant="outline">端口 {port}</Badge>
          </div>
          <div className="flex gap-2">
            <Button size="sm" onClick={openWebGui}>
              <MonitorUp /> 内嵌打开
            </Button>
            <Button size="sm" variant="outline" onClick={openExternal}>
              <ExternalLink /> 外部浏览器
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
