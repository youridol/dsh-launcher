// 状态卡片：dsh 运行状态 + 生命周期控制 + 端口配置 + Web GUI（整合）
// v0.4.11：内嵌打开优化——dsh 未运行时自动拉起并等待就绪，期间弹进度小窗展示阶段。
import { useCallback, useEffect, useRef, useState } from "react";
import { useRefreshOnEvent } from "@/hooks/useTauriEvent";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CardDescription, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { Progress } from "@/components/ui/progress";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  createDesktopShortcut,
  createWebGuiWindow,
  getDshStatus,
  getConfig,
  getInstalledVersion,
  getWebUrl,
  isDshManaged,
  listenVersionChanged,
  probeWebReady,
  restartDsh,
  setPort as savePort,
  startDsh,
  stopDsh,
  takeOverDsh,
  uninstallDsh,
  type DshStatus as Status,
} from "@/lib/tauri";
import { toast } from "sonner";
import {
  Play,
  Square,
  RotateCw,
  Activity,
  ExternalLink,
  MonitorUp,
  MonitorDown,
  Trash2,
  Loader2,
  CheckCircle2,
  XCircle,
} from "lucide-react";

// 状态徽标配色
const STATUS_META: Record<Status, { label: string; variant: "default" | "secondary" | "destructive" | "outline" }> = {
  stopped: { label: "未运行", variant: "outline" },
  starting: { label: "启动中", variant: "secondary" },
  running: { label: "运行中", variant: "default" },
  stopping: { label: "停止中", variant: "secondary" },
  error: { label: "出错", variant: "destructive" },
};

// 内嵌打开引导弹窗的阶段
const OPEN_STAGES = [
  "检查 dsh 状态…",
  "正在启动 dsh…",
  "等待 dsh 就绪（端口监听）…",
  "获取访问地址…",
  "打开内嵌窗口…",
] as const;

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

  // —— 内嵌打开进度弹窗状态 ——
  // open-guide 弹窗：opening=true 展示中；stageIdx=当前阶段；err=失败信息（非空时展示失败态）
  const [opening, setOpening] = useState(false);
  const [stageIdx, setStageIdx] = useState(0);
  const [openErr, setOpenErr] = useState<string | null>(null);
  // v0.9.1：失败原因为「外部实例无法取得令牌」时置位 → 弹窗多给一个「接管并重启」
  const [needsTakeover, setNeedsTakeover] = useState(false);
  // 当前打开模式（embedded=内嵌窗口 / external=外部浏览器）
  const openMode = useRef<"embedded" | "external">("embedded");
  // 打开流程守卫：取消/重开时使旧流程的异步 setState 失效（避免竞态污染）
  const openSeq = useRef(0);

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
  // 统一走 useTauriEvent（ADR-0009 D7）：竞态安全的解绑，不再手写 unlisten?.()
  useRefreshOnEvent(listenVersionChanged, () => {
      getInstalledVersion()
        .then((v) => setHasInstalled(Boolean(v)))
        .catch(() => setHasInstalled(false));
      refresh();
    },
    [refresh],
  );

  async function handleStart() {
    setBusy(true);
    // v0.5.1：点启动立即把状态置 starting（不再等轮询），提供即时状态反馈
    setStatus("starting");
    try {
      // 先保存端口再启动
      const p = Number(port);
      if (!Number.isInteger(p) || p < 1 || p > 65535) {
        toast.error("端口不合法（1-65535）");
        setStatus("stopped");
        return;
      }
      await savePort(p);
      // v0.5.1：startDsh 内部等待端口就绪后返回，回来即置 running
      await startDsh();
      setStatus("running");
      toast.success("dsh 已启动");
      // v0.5.1（产品优化）：启动成功后自动打开内嵌 Web GUI。
      // openWithGuide 内部：running 态会跳过重复启动，直接等端口就绪 →
      // 等带 token URL → 创建内嵌窗口（复用 Rust 创建路径，图标清晰）。
      // 与 autoOpenBrowser 开关语义一致（该开关控制的是启动器启动时的自动打开）。
      void openWithGuide("embedded");
    } catch (e) {
      toast.error(`启动失败: ${e}`);
      refresh();
    } finally {
      setBusy(false);
      // 兜底同步真实状态（防乐观置位与实际不符）
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
      // v0.4.15（审计修复）：重启前先保存输入框端口 —— Rust restart 用当前生效端口
      // （旧端口）重启，若不保存则 UI 显示新端口、实际重启到旧端口，产生误导。
      const p = Number(port);
      if (!Number.isInteger(p) || p < 1 || p > 65535) {
        toast.error("端口不合法（1-65535）");
        return;
      }
      await savePort(p);
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

  async function handleDesktopShortcut() {
    try {
      const msg = await createDesktopShortcut();
      toast.success(msg);
    } catch (e) {
      toast.error(`创建桌面快捷方式失败: ${e}`);
    }
  }

  async function openWebGui() {
    await openWithGuide("embedded");
  }

  async function openExternal() {
    await openWithGuide("external");
  }

  /**
   * 统一打开引导流程（v0.4.11 优化）：
   * 1. 弹进度小窗（阶段化展示）；
   * 2. dsh 未运行（stopped/error）→ 自动 startDsh() 拉起（不再要求手动先启动）；
   * 3. 轮询状态至 running（等端口监听）→ 轮询获取带 token 完整 URL；
   * 4. 就绪 → 关弹窗 → 内嵌窗口 / 外部浏览器；
   * 5. 任一步失败/超时 → 弹窗转失败态（含原因），可重试/关闭。
   */
  async function openWithGuide(mode: "embedded" | "external", force = false) {
    if (guiBusy && !force) return;
    openMode.current = mode;
    const seq = ++openSeq.current; // 本流程序号（旧流程异步回来时 seq 不匹配 → 丢弃）
    // 仅以序号判断“仍是当前流程”。不能用 opening state（闭包捕获的是初始值）。
    const alive = () => openSeq.current === seq;
    setGuiBusy(true);
    setOpenErr(null);
    setNeedsTakeover(false);
    setStageIdx(0);
    setOpening(true);
    try {
      // ① 检查是否已安装 dsh（未安装则启动必然失败，提前提示）
      let installed = false;
      try {
        installed = Boolean(await getInstalledVersion());
      } catch {
        installed = false;
      }
      if (!alive()) return;
      if (!installed) {
        setOpenErr("未检测到已安装的 dsh，请先在「版本管理」中安装后再打开");
        return;
      }

      // ② 检查运行状态；非 running → 自动启动
      let st: Status = "stopped";
      try {
        st = await getDshStatus();
      } catch (e) {
        console.error("获取状态失败", e);
      }
      if (st === "stopped" || st === "error") {
        // 自动拉起 dsh（端口取当前输入框配置；保存端口由 handleStart 语义一致）
        setStageIdx(1);
        const p = Number(port);
        if (!Number.isInteger(p) || p < 1 || p > 65535) {
          setOpenErr("端口不合法（1-65535），请先在设置中修正");
          return;
        }
        try {
          await savePort(p);
          await startDsh();
        } catch (e) {
          if (!alive()) return;
          setOpenErr(`启动 dsh 失败：${e}`);
          return;
        }
        refresh();
      } else if (st === "stopping") {
        // 停止中 → 等它停完再启动（最多 8s）
        setStageIdx(1);
        const deadline = Date.now() + 8_000;
        while (Date.now() < deadline) {
          try {
            const s2 = await getDshStatus();
            if (s2 === "stopped") break;
            if (s2 === "running") break;
          } catch {
            /* 忽略瞬时错误继续轮询 */
          }
          if (!alive()) return;
          await sleep(400);
        }
        try {
          st = await getDshStatus();
        } catch {
          st = "stopped";
        }
        if (!alive()) return;
        if (st === "stopped" || st === "error") {
          setStageIdx(1);
          try {
            await startDsh();
          } catch (e) {
            if (!alive()) return;
            setOpenErr(`启动 dsh 失败：${e}`);
            return;
          }
          refresh();
        }
      }

      // ③ 等 dsh 就绪（running = 端口监听中）。
      // v0.9.1（启动未就绪 BUG 修复）：本阶段此前是**唯一的失败点**，且文案把一切
      // 失败都说成"端口未监听"——实测端口其实已监听（冷启动 8.83s），只是后端状态机
      // 卡在 Starting 未收敛。现在：后端已有对账兜底 + start_dsh 观测即置位，
      // 这里同时放宽到 40s（覆盖插件/MCP 拉长的冷启动），并按**真实原因**分别报错。
      setStageIdx(2);
      const readyDeadline = Date.now() + 40_000;
      let ready = false;
      while (Date.now() < readyDeadline) {
        try {
          const s = await getDshStatus();
          if (s === "running") {
            ready = true;
            break;
          }
          if (s === "stopped" || s === "error") break; // 进程已退出，不再等待
        } catch {
          /* 瞬时错误继续 */
        }
        if (!alive()) return;
        await sleep(500);
      }
      if (!alive()) return;
      if (!ready) {
        // 按真实状态给出可操作的原因，不再一律误报"端口未监听"
        let finalStatus: Status = "stopped";
        try {
          finalStatus = await getDshStatus();
        } catch {
          /* 保持默认 */
        }
        if (!alive()) return;
        if (finalStatus === "stopped" || finalStatus === "error") {
          setOpenErr(
            "dsh 进程已退出，未能就绪。请查看下方日志面板定位原因（常见：端口被占用、插件加载失败），或先停止再启动",
          );
        } else {
          setOpenErr(
            "dsh 进程仍在启动但未在 40 秒内就绪。请查看日志面板确认进度，稍后点「重试」，或先停止再启动",
          );
        }
        return;
      }

      // v0.5.7（启动自动开窗修复）：URL 获取与 HTTP 就绪探测**一体化循环**。
      // 旧实现：waitForWebUrl 一次拿到 url 即固定，再用同一 url 探测 30s——若该
      // url 是旧/过期 token（dsh 重启后 token 变化、或启动瞬间拿到残留值），dsh
      // 对无效 token 返回 **400 Bad Request**（非 2xx/3xx）→ 探测 30s 死循环超时，
      // 而手动"内嵌打开"时 dsh 稳定、url 为当前有效 token（303）→ 秒过。
      // 现在：循环内每次先取**最新** getWebUrl（token 更新后自然拿到新值），有
      // 有效 URL 才探测；探测通过即开窗，不通过继续取最新 URL 重试（≤40s）。
      // 阶段推进到「获取访问地址」（索引 3）：此前直接跳到索引 4，
      // 使 OPEN_STAGES[3] 永不展示、进度条出现跳变（ADR-0009 D19）
      setStageIdx(3);
      const httpDeadline = Date.now() + 40_000;
      let finalUrl: string | null = null;
      // v0.9.1：判断是否"已运行但 token 原理上不可得"（收养的外部实例）。
      // 外部实例的 token 只从**它自己的** stdout 打印，启动器拿不到；旧实现会在
      // 这里空转 40 秒后报"访问地址无效"，用户无从下手。现在辨识该情形并给出
      // **可操作的接管选择**（停止后由启动器重新拉起 → token 可被捕获）。
      let externalNoToken = false;
      let sawUrl = false;
      while (Date.now() < httpDeadline) {
        if (!alive()) return;
        // 每次取最新 URL（token 会随 dsh 重启变化）
        let cur: string | null = null;
        try {
          cur = await getWebUrl();
        } catch {
          cur = null;
        }
        // 仅含 token 的 URL 才参与探测
        if (cur && /[?&]token=/.test(cur)) {
          sawUrl = true;
          try {
            if (await probeWebReady(cur)) {
              finalUrl = cur;
              break;
            }
          } catch {
            /* 瞬时失败继续 */
          }
        } else {
          // 没有可用 URL：若是**非托管**实例，再等也不会出现（token 不可得）→ 提前决策
          try {
            if (!(await isDshManaged())) {
              externalNoToken = true;
              break;
            }
          } catch {
            /* 查询失败按托管处理，继续等待 */
          }
        }
        await sleep(700);
      }
      if (!alive()) return;
      if (!finalUrl && externalNoToken) {
        // 外部实例：询问是否由启动器接管（会中断该实例上的会话）
        setOpenErr(
          "当前 dsh 是**在启动器之外**启动的实例，它的访问令牌只在该进程自己的输出里，启动器无法获取，因此无法免认证打开内嵌窗口。\n可点「接管并重启」：停止该实例并由启动器重新启动（会中断其中正在进行的会话）。",
        );
        setNeedsTakeover(true);
        return;
      }
      if (!finalUrl) {
        setOpenErr(
          sawUrl
            ? "dsh 已监听端口但访问地址未在 40 秒内通过 HTTP 就绪探测（可能 token 过期或服务仍在预热）。请点「重试」，或先停止再启动"
            : "dsh 运行中但未输出访问地址（含 token）。请查看日志面板；若为启动器之外启动的实例，请先停止它再由启动器启动",
        );
        return;
      }
      const url = finalUrl;
      // 缓冲 300ms 让服务稳定后 WebView2 首载
      await sleep(300);
      if (!alive()) return;

      // 阶段推进到「打开内嵌窗口」（最后一阶段）
      setStageIdx(4);

      // ⑤ 打开目标
      if (mode === "embedded") {
        // v0.4.15 起统一走 Rust 创建内嵌窗口（create_web_gui_window）——
        // 窗口在 Tauri 主线程以"builder 预置 512px 图标 + 创建即 SMALL/BIG"创建，
        // 杜绝 `new WebviewWindow` 首帧使用默认 exe 16px 图标导致的任务栏模糊。
        // v0.5.2：命令改为主线程建窗、不等待返回（消除跨线程阻塞死锁 → 白屏/卡死），
        // 图标在创建时已设置，无需 label 兜底补发。
        try {
          await createWebGuiWindow(url);
        } catch (e) {
          console.error("创建内嵌窗口失败", e);
          toast.error(`打开内嵌 Web GUI 失败: ${e}`);
          return;
        }
      } else {
        await openUrl(url);
      }
      if (!alive()) return;
      // 成功 → 关弹窗（递增 seq 使本流程后续无效）
      openSeq.current++;
      setOpening(false);
      setGuiBusy(false);
    } catch (e) {
      console.error("打开 Web GUI 失败", e);
      if (!alive()) return;
      setOpenErr(`打开失败：${e}`);
    }
  }

  /** 弹窗关闭（手动/成功后）时复位 busy 态 */
  function closeOpenGuide() {
    openSeq.current++; // 使进行中的流程失效（其后续 await 回来不再 setState）
    setOpening(false);
    setGuiBusy(false);
    setOpenErr(null);
    setNeedsTakeover(false);
  }

  /**
   * 接管外部启动的 dsh（v0.9.1）：停止该实例并由启动器重新拉起，使 token 可被捕获。
   * 由用户在失败弹窗中显式点击触发（会中断该实例上的会话）。
   */
  async function handleTakeover() {
    setNeedsTakeover(false);
    setOpenErr(null);
    setStageIdx(1);
    try {
      await takeOverDsh();
    } catch (e) {
      setOpenErr(`接管失败：${e}`);
      return;
    }
    // 重启命令返回后再走一次完整引导流程（此时实例已由启动器托管）
    await sleep(300);
    await openWithGuide(openMode.current, true);
  }

  function sleep(ms: number) {
    return new Promise((r) => setTimeout(r, ms));
  }

  return (
    <div className="flex flex-col">
      {/* 标题块（无卡片外壳：直接作为侧边栏面板内容） */}
      <div className="flex flex-col gap-1">
        <CardTitle className="flex flex-wrap items-center gap-2">
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
          {/* 生命周期按钮组：窄宽度下自动换行，不溢出侧栏 */}
          <div className="flex min-w-0 flex-wrap gap-2">
            <Button onClick={handleStart} disabled={running || busy}>
              {busy && status === "starting" ? <Loader2 className="size-4 animate-spin" /> : <Play />}
              {busy && status === "starting" ? "启动中…" : "启动"}
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
          <div className="flex flex-wrap items-center gap-2">
            <MonitorUp className="size-4 text-muted-foreground" />
            <span className="text-sm font-medium">Web GUI</span>
            <Badge variant="outline">端口 {port}</Badge>
          </div>
          <div className="flex flex-wrap gap-2">
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

      {/* 打开 Web GUI 进度引导弹窗（v0.4.11：自动拉起 + 就绪后才打开，期间展示进度） */}
      <Dialog
        open={opening}
        onOpenChange={(o) => {
          // 用户手动关闭（点遮罩/关闭按钮）→ 复位；失败态可关闭
          if (!o) closeOpenGuide();
        }}
      >
        <DialogContent showCloseButton={false} className="w-[calc(100vw-2rem)] max-w-xs">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              {openErr ? (
                <XCircle className="size-4 text-destructive" />
              ) : stageIdx === OPEN_STAGES.length - 1 && !openErr ? (
                <CheckCircle2 className="size-4 text-emerald-500" />
              ) : (
                <Loader2 className="size-4 animate-spin" />
              )}
              {openErr
                ? "打开失败"
                : stageIdx === OPEN_STAGES.length - 1
                  ? "正在打开…"
                  : "正在打开 Web GUI"}
            </DialogTitle>
            <DialogDescription className="pt-1 text-xs">
              {openErr
                ? "未能打开 dsh Web GUI"
                : openMode.current === "embedded"
                  ? "dsh 就绪后将打开内嵌窗口"
                  : "dsh 就绪后将打开外部浏览器"}
            </DialogDescription>
          </DialogHeader>

          {/* 主体：进度 / 失败态（DialogBody 提供统一内边距与滚动） */}
          <DialogBody className="space-y-3">
            {/* 进度：阶段条（0~4 映射 0~100）+ 当前阶段文案 */}
            {!openErr && (
              <div className="space-y-2">
                <Progress
                  value={Math.min(100, Math.round((stageIdx / (OPEN_STAGES.length - 1)) * 100))}
                  className="h-1.5"
                />
                <div className="flex items-center gap-2 text-xs text-muted-foreground">
                  {stageIdx === OPEN_STAGES.length - 1 ? (
                    <Loader2 className="size-3 animate-spin" />
                  ) : (
                    <span className="size-3 shrink-0" />
                  )}
                  <span className="truncate">{OPEN_STAGES[stageIdx]}</span>
                </div>
              </div>
            )}

            {/* 失败提示 + 重试/关闭 */}
            {openErr && (
              <div className="space-y-3">
                <p className="break-words whitespace-pre-line text-xs text-destructive">{openErr}</p>
                <div className="flex flex-wrap justify-end gap-2">
                  <Button variant="outline" size="sm" onClick={closeOpenGuide}>
                    关闭
                  </Button>
                  {needsTakeover && (
                    <Button variant="secondary" size="sm" onClick={() => void handleTakeover()}>
                      <RotateCw className="size-3" /> 接管并重启
                    </Button>
                  )}
                  <Button
                    size="sm"
                    onClick={() => {
                      // 重试（force：失败态 guiBusy=true 也放行，重新走引导流程）
                      setOpenErr(null);
                      setNeedsTakeover(false);
                      void openWithGuide(openMode.current, true);
                    }}
                  >
                    <RotateCw className="size-3" /> 重试
                  </Button>
                </div>
              </div>
            )}

            {/* 等待中：允许取消（不阻塞，用户可关闭去手动处理） */}
            {!openErr && (
              <Button variant="ghost" size="sm" className="self-end" onClick={closeOpenGuide}>
                取消
              </Button>
            )}
          </DialogBody>
        </DialogContent>
      </Dialog>
    </div>
  );
}
