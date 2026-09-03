// 状态卡片：dsh 运行状态 + 生命周期控制 + 端口配置 + Web GUI（整合）
import { useCallback, useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CardDescription, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  createDesktopShortcut,
  getDshStatus,
  getConfig,
  getInstalledVersion,
  getWebUrl,
  listenVersionChanged,
  restartDsh,
  setPort as savePort,
  setWebGuiIcon,
  startDsh,
  stopDsh,
  uninstallDsh,
  type DshStatus as Status,
} from "@/lib/tauri";
import { toast } from "sonner";
import { Play, Square, RotateCw, Activity, ExternalLink, MonitorUp, MonitorDown, Trash2 } from "lucide-react";

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
  // Web GUI 打开中（等待 URL 期间禁用按钮防重复点击）
  const [guiBusy, setGuiBusy] = useState(false);
  // 是否已安装 dsh（卸载按钮可用性）
  const [hasInstalled, setHasInstalled] = useState(false);
  // 卸载进行中（独立于启动/停止 busy，避免互斥）
  const [uninstallBusy, setUninstallBusy] = useState(false);

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

  // 查询当前安装状态（卸载按钮禁用依据）
  useEffect(() => {
    getInstalledVersion()
      .then((v) => setHasInstalled(Boolean(v)))
      .catch(() => setHasInstalled(false));
  }, []);

  // 订阅 dsh 安装版本变更事件：卸载/安装完成后刷新安装状态 + 运行状态
  // （Rust 端在 uninstall/install 成功后广播 version://changed）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listenVersionChanged(() => {
          getInstalledVersion()
            .then((v) => setHasInstalled(Boolean(v)))
            .catch(() => setHasInstalled(false));
          refresh();
        });
      } catch (e) {
        console.error("订阅版本变更事件失败", e);
      }
    })();
    return () => {
      unlisten?.();
    };
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

  async function handleUninstall() {
    setUninstallBusy(true);
    try {
      const msg = await uninstallDsh();
      toast.success(msg);
      // 卸载后刷新安装状态与运行状态
      getInstalledVersion()
        .then((v) => setHasInstalled(Boolean(v)))
        .catch(() => setHasInstalled(false));
      refresh();
    } catch (e) {
      toast.error(`卸载失败: ${e}`);
    } finally {
      setUninstallBusy(false);
    }
  }

  const running = status === "running" || status === "starting";

  // Web GUI：内嵌 Tauri WebviewWindow / 外部浏览器（opener 插件）
  // 用带 token 的完整 URL 打开（dsh web 要求认证，裸 URL 会 401）。
  // 分发机器修复（v0.4.4）：dsh 冷启动后 token 需要数秒~数十秒才从 stdout
  // 实时到达启动器。这里轮询等待完整 URL（最多 40s），拿不到就不打开窗口，
  // 避免两种"死窗口"：端口未监听 → ERR_CONNECTION_REFUSED；
  // 裸 URL → 401 "dsh web authentication required"。
  async function waitForWebUrl(timeoutMs: number): Promise<string | null> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      try {
        const full = await getWebUrl();
        if (full) return full;
      } catch (e) {
        console.error("获取 Web URL 失败（重试）", e);
      }
      await new Promise((r) => setTimeout(r, 500));
    }
    return null;
  }

  async function openWebGui() {
    if (guiBusy) return;
    setGuiBusy(true);
    try {
      if (status === "stopped" || status === "error") {
        toast.error("dsh 未在运行，请先点击「启动」");
        return;
      }
      const url = await waitForWebUrl(40_000);
      if (!url) {
        toast.error("尚未获取 dsh Web 访问地址（token 未输出）。dsh 可能仍在启动，请稍后重试；若持续如此请先停止再启动 dsh");
        return;
      }
      // 每次使用唯一 label 创建（修复：窗口关闭后 getByLabel 残留导致无法重建）
      const label = `dsh-web-gui-${Date.now()}`;
      const win = new WebviewWindow(label, { url, title: "deepseek-harness Web UI", width: 1600, height: 900 });
      // 创建失败提示（Tauri 2 创建错误通过 tauri://error 事件上报）
      win.once("tauri://error", (e) => {
        console.error("内嵌窗口创建失败", e);
        toast.error(`打开内嵌 Web GUI 失败: ${e.payload}`);
      });
      // 窗口创建成功后设置高清任务栏图标（修复模糊：子窗口默认图标为低分辨率缩放）
      // 需等 tauri://created（创建完成窗口才可被 Rust 端查到）
      win.once("tauri://created", () => {
        setWebGuiIcon(label).catch((e) => {
          console.error("设置内嵌窗口图标失败", e);
        });
      });
    } catch (e) {
      console.error("打开内嵌 Web GUI 失败", e);
      toast.error(`打开内嵌 Web GUI 失败: ${e}`);
    } finally {
      setGuiBusy(false);
    }
  }

  async function handleDesktopShortcut() {
    try {
      const msg = await createDesktopShortcut();
      toast.success(msg);
    } catch (e) {
      toast.error(`创建桌面快捷方式失败: ${e}`);
    }
  }

  async function openExternal() {
    if (guiBusy) return;
    setGuiBusy(true);
    try {
      if (status === "stopped" || status === "error") {
        toast.error("dsh 未在运行，请先点击「启动」");
        return;
      }
      // 用 opener 插件打开系统浏览器（window.open 在 Tauri 沙箱内被拦截 → 无反应）
      const url = await waitForWebUrl(40_000);
      if (!url) {
        toast.error("尚未获取 dsh Web 访问地址（token 未输出）。dsh 可能仍在启动，请稍后重试；若持续如此请先停止再启动 dsh");
        return;
      }
      await openUrl(url);
    } catch (e) {
      console.error("打开外部浏览器失败", e);
      toast.error(`打开外部浏览器失败: ${e}`);
    } finally {
      setGuiBusy(false);
    }
  }

  return (
    <div className="flex flex-col">
      {/* 标题块（无卡片外壳：直接作为侧边栏面板内容） */}
      <div className="flex flex-col gap-1">
        <CardTitle className="flex items-center gap-2">
          <Activity className="size-4" />
          dsh 运行状态
          <Badge variant={STATUS_META[status].variant}>{STATUS_META[status].label}</Badge>
        </CardTitle>
        <CardDescription>dsh 生命周期与 Web GUI</CardDescription>
      </div>
      <Separator className="my-3" />
      <div className="space-y-4">
        {/* 布局：允许换行，窄宽侧栏下不溢出（仅布局属性） */}
        <div className="flex flex-wrap items-end gap-4">
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
            {/* 卸载（从版本管理面板移入；置于重启右侧） */}
            <Button
              variant="destructive"
              onClick={handleUninstall}
              disabled={uninstallBusy || !hasInstalled}
              title="卸载 dsh（移除代码，DSH_HOME 数据默认保留）"
            >
              <Trash2 /> {uninstallBusy ? "卸载中…" : "卸载"}
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
            <Button size="sm" onClick={openWebGui} disabled={guiBusy} title={guiBusy ? "正在等待 dsh Web 就绪（获取访问地址）…" : undefined}>
              <MonitorUp /> {guiBusy ? "等待 Web 就绪…" : "内嵌打开"}
            </Button>
            <Button size="sm" variant="outline" onClick={openExternal} disabled={guiBusy}>
              <ExternalLink /> 外部浏览器
            </Button>
            <Button size="sm" variant="outline" onClick={handleDesktopShortcut}>
              <MonitorDown /> 发送到桌面
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
