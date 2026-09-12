// 日志侧边栏：全域日志流式展示（dsh + 启动器），固定在右侧，无卡片容器
// 实时流：Tauri event `log://line`（Rust logging.rs emit）
// 补流：挂载时读取最新日志文件（含启动早期日志与 dsh 日志）→ 放入实时流，避免丢失
//
// v0.4.13（审计修复 M2/L6/L9）：
// - M2：每条日志只做一次 emoji 清理 + 一次格式化，缓存于 entries.text；
//   streamText 改为 useMemo(entries)，不再每条事件全缓冲重扫正则（O(n²)→O(新行)）；
// - L6：仅当用户停留在顶部附近时才自动回顶，不与用户滚动打架；
// - L9：导出 blob URL 延迟回收；文件名路径分隔符统一清洗；
// - L7：移除无调用方的 onClose prop。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { listLogs, readLog, type LogFile } from "@/lib/tauri";
import { listen } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { toast } from "sonner";
import { Download, Eraser, ShieldOff } from "lucide-react";

// 日志行（对应 Rust LogLine）
interface LogLine {
  timestamp: string;
  source: string;
  level: string;
  message: string;
}

// 缓冲条目：原始行 + 已格式化（去 emoji）文本（格式化只做一次）
interface StreamEntry {
  raw: LogLine;
  text: string;
}

// 实时流最大缓冲行数
const MAX_STREAM_LINES = 2000;

// 解析落盘日志行 "[HH:MM:SS] [source] [LEVEL] message" → LogLine
function parseLogLine(line: string): LogLine | null {
  const m = line.match(/^\[(\d{2}:\d{2}:\d{2})\] \[(\w+)\] \[(\w+)\] (.*)$/);
  if (!m) return null;
  return {
    timestamp: m[1],
    source: m[2],
    level: m[3],
    message: m[4],
  };
}

// 移除日志消息中的 emoji（dsh 终端输出 emoji 在跨编码转换时会产生乱码，直接剔除）
function stripEmoji(s: string): string {
  // 匹配常见 Unicode emoji 区段 + 变体选择符 + ZWJ 序列（保留 ASCII/中文）
  return s.replace(
    /[\u{1F000}-\u{1FAFF}\u{2600}-\u{27BF}\u{FE0F}\u{1F1E6}-\u{1F1FF}\u{200D}\u{2B00}-\u{2BFF}\u{2B50}\u{3030}\u{00A9}\u{00AE}]/gu,
    "",
  );
}

function formatLogLine(l: LogLine): string {
  return stripEmoji(`[${l.timestamp}] [${l.source}] [${l.level}] ${l.message}`);
}

/**
 * 把日志文本中的 dsh 访问 token 打码（G5 / ADR-0009 D5）。
 *
 * 仅用于**导出产物**（供外发/上报）；屏幕展示与落盘保持 ADR-0009 D5 的明文口径。
 * 覆盖两类出现形式：
 * - 查询串 `token=<值>`（含 `?token=` 与 `&token=`）；
 * - 裸 `token=` 后接 URL-safe 值。
 * 只替换值本身，保留 `token=` 便于阅读时定位。
 */
function redactWebTokens(text: string): string {
  return text.replace(/(token=)[^\s&"'\\]+/gi, "$1***");
}

function toEntry(l: LogLine): StreamEntry {
  return { raw: l, text: formatLogLine(l) };
}

export default function LogPanel({ className }: { className?: string }) {
  const [logs, setLogs] = useState<LogFile[]>([]);
  const [content, setContent] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  // 实时流缓冲（最新在最前）
  const [entries, setEntries] = useState<StreamEntry[]>([]);
  // 是否为"实时流"视图
  const [liveMode, setLiveMode] = useState(true);
  // 内容容器引用（自动滚动；指向 ScrollArea 的 viewport，即真正的滚动容器）
  const scrollRef = useRef<HTMLDivElement>(null);
  // 已补流的日志文件（避免重复加载同一文件追加）
  const backfilledRef = useRef<string | null>(null);
  // 用户是否停留在"顶部（最新）附近"——仅在此时自动回顶（L6）
  const stickToTopRef = useRef(true);

  // 订阅 Rust 日志事件（实时流，最新条目在最前）
  // 统一走 useTauriEvent（ADR-0009 D7）：本处原为全仓唯一写对的实现，
  // 现抽出为共用封装，此处改为调用它（行为等价：竞态安全 + 不吞错误）
  useTauriEvent<LogLine>(
    (onPayload) => listen<LogLine>("log://line", (event) => onPayload(event.payload)),
    (payload) => {
      setEntries((prev) => {
        const next = [toEntry(payload), ...prev];
        return next.length > MAX_STREAM_LINES ? next.slice(0, MAX_STREAM_LINES) : next;
      });
    },
    [],
  );

  // 滚动位置跟踪：靠近顶部（最新）视为"贴顶"
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      stickToTopRef.current = el.scrollTop < 60;
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  // 最新在上 → 仅当用户贴顶时自动回顶（L6：不打断向下阅读）
  useEffect(() => {
    if (liveMode && stickToTopRef.current && scrollRef.current) {
      scrollRef.current.scrollTop = 0;
    }
  }, [entries, liveMode]);

  // 补流：读取最新日志文件 → 解析为条目（倒序：新在前）放入实时流
  // 覆盖 LogPanel 挂载前产生的日志（如启动早期、主窗口操作、dsh 启动输出）
  // v0.4.15（审计修复）：只补流 <date>.log（排除 dsh-web-stdout/stderr.log——
  // 那些是进程输出落盘，已实时逐行进 log://line 流，补流会与实时流大量重复），
  // 且只读尾部 MAX_STREAM_LINES 行（避免一次性全量解析 10MB 大文件卡首帧）。
  const backfillLatestFile = useCallback(async () => {
    try {
      const files = await listLogs();
      // 只取日期日志：yyyy-MM-dd.log（切割档 .log.1 是历史，不补）
      const latest = files.find((f) => /^\d{4}-\d{2}-\d{2}\.log$/.test(f.path));
      if (!latest || backfilledRef.current === latest.path) return;
      const rawText = await readLog(latest.path);
      const lines: LogLine[] = [];
      // 从后向前解析并截断到 MAX_STREAM_LINES（只保留最新 N 行）
      for (const line of rawText.split(/\r?\n/).reverse()) {
        const parsed = parseLogLine(line);
        if (parsed) {
          lines.push(parsed);
          if (lines.length >= MAX_STREAM_LINES) break;
        }
      }
      if (lines.length > 0) {
        backfilledRef.current = latest.path;
        setEntries((prev) => {
          // 合并去重：跳过与已缓冲相同 timestamp+message 的旧条目
          const seen = new Set(
            prev.slice(0, 50).map((e) => e.raw.timestamp + e.raw.message),
          );
          const added: StreamEntry[] = [];
          for (const l of lines) {
            if (!seen.has(l.timestamp + l.message)) {
              seen.add(l.timestamp + l.message);
              added.push(toEntry(l));
            }
          }
          const merged = [...prev, ...added];
          return merged.slice(0, MAX_STREAM_LINES);
        });
      }
    } catch (e) {
      console.error("补流日志失败", e);
    }
  }, []);

  // 文件读取的单调序号守卫（ADR-0009 D10）。
  //
  // 症状：`setSelected(path); setContent(await readLog(path))` 无守卫时，快速连点两个文件会
  // 让**先发起但更慢**的 readLog 结果覆盖后选文件的内容 —— 标题显示 A、正文是 B。
  // 修法：每次发起读取前自增序号；await 返回后只在「序号仍是最新」时落 state，
  // 过期结果直接丢弃。`refreshFiles` 的自动选中走同一条路径，因此共用同一守卫。
  const readSeqRef = useRef(0);

  /** 读取并展示某个日志文件；过期结果不落 state */
  const loadLogContent = useCallback(async (path: string) => {
    const seq = ++readSeqRef.current;
    try {
      const text = await readLog(path);
      if (seq !== readSeqRef.current) return; // 已有更新的选择，丢弃本次结果
      setContent(text);
    } catch (e) {
      if (seq !== readSeqRef.current) return;
      toast.error(`读取日志失败: ${e}`);
    }
  }, []);

  // 文件列表刷新（最新文件优先选中展示）
  const refreshFiles = useCallback(async () => {
    try {
      const files = await listLogs();
      setLogs(files);
      if (files.length > 0 && !selected) {
        const first = files[0];
        setSelected(first.path);
        await loadLogContent(first.path);
      }
    } catch (e) {
      console.error("读取日志列表失败", e);
    }
  }, [selected, loadLogContent]);

  // 让 30s 轮询始终调用最新的 refreshFiles（避免因 selected 变化反复重建定时器）
  const refreshRef = useRef(refreshFiles);
  refreshRef.current = refreshFiles;

  useEffect(() => {
    // 首次挂载：先补流最新日志（含启动早期/dsh 日志），再刷新文件列表
    backfillLatestFile().finally(() => refreshRef.current());
    // 实时流为主：文件列表定期刷新（30 秒）保持文件视图可用
    const timer = setInterval(() => {
      refreshRef.current();
    }, 30000);
    return () => clearInterval(timer);
  }, [backfillLatestFile]);

  async function selectLog(path: string) {
    setLiveMode(false);
    setSelected(path);
    // 经序号守卫读取：快速连点多个文件时，过期结果不会覆盖后选文件（ADR-0009 D10）
    await loadLogContent(path);
  }

  function exportLog() {
    // 实时流模式导出流内容（text 已去 emoji）；文件模式导出文件内容（导出前剔除 emoji）
    const data = liveMode
      ? entries.map((e) => e.text).join("\n")
      : stripEmoji(content);
    downloadText(data, liveMode ? "dsh-live.log" : `${selected?.replace(/[\\/]/g, "-") ?? "dsh.log"}`);
  }

  /**
   * 导出**打码版**日志（G5 / ADR-0009 D5）。
   *
   * ADR-0009 D5 保留了「日志与前端流明文含 token」的产品决策（用户需复制完整
   * 带 token 地址在外部浏览器打开），但同节亦明确指出：向外部分享/上报日志时应
   * 提供**打码**路径。此处只影响导出产物，**不改变**默认的屏幕展示与落盘口径。
   */
  function exportLogRedacted() {
    const data = liveMode
      ? entries.map((e) => e.text).join("\n")
      : stripEmoji(content);
    const redacted = redactWebTokens(data);
    if (redacted === data) {
      toast.info("本段日志未发现访问 token，已按原样导出");
    } else {
      toast.success("已导出打码版日志（token 已替换为 ***）");
    }
    const name = liveMode ? "dsh-live-redacted.log" : `redacted-${selected?.replace(/[\\/]/g, "-") ?? "dsh.log"}`;
    downloadText(redacted, name);
  }

  /** 把文本下载为本地文件（统一 blob URL 的创建与延后回收）。 */
  function downloadText(data: string, filename: string) {
    const blob = new Blob([data], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    a.click();
    // 延后回收 blob URL（避免下载开始前被作废）
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }

  // 清屏：清空实时流缓冲（文件列表仍由定时器自动刷新）
  function clearLog() {
    setEntries([]);
    setContent("");
    // 重置补流标记：下次手动切回实时流不自动补历史（清屏意图 = 只看后续新日志）
    backfilledRef.current = null;
  }

  const streamText = useMemo(
    () => entries.map((e) => e.text).join("\n"),
    [entries],
  );

  return (
    // 子窗口外观：独立边框 + 背景 + 圆角，标题栏与内容区分离
    <div
      className={cn(
        "flex h-full flex-col overflow-hidden", // 全页展示：去除卡片/子窗口外壳（边框/圆角/阴影/背景）
        className,
      )}
    >
      {/* 标题栏（一体化侧边栏：无背景色、无描边） */}
      <div className="flex shrink-0 items-center justify-between gap-2 px-4 py-2">
        <div className="flex items-center gap-2">
          <span className="text-sm font-semibold">日志</span>
          <Button
            variant={liveMode ? "default" : "ghost"}
            size="xs"
            onClick={() => {
              setLiveMode(true);
              setSelected(null);
            }}
          >
            实时流
          </Button>
        </div>
        <div className="flex items-center gap-1">
          {/* 清屏（清空实时流缓冲，文件列表仍由定时器自动刷新） */}
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={clearLog}
            title="清屏"
            aria-label="清空日志流"
          >
            <Eraser />
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={exportLog}
            title="导出日志"
            aria-label="导出日志"
          >
            <Download />
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={exportLogRedacted}
            title="导出打码版日志（token → ***，供外发/上报）"
            aria-label="导出打码版日志"
          >
            <ShieldOff />
          </Button>
        </div>
      </div>

      {/* 内容区（一体化侧边栏：无背景色、无描边；右侧不留 padding，滚动条紧贴右缘） */}
      <div className="flex min-h-0 flex-1 gap-3 pl-4 pt-1 pb-2">
        {/* 日志文件列表（非实时流时显示）：入场淡入，回答"视图切换了" */}
        {!liveMode && (
          <div className="dsh-fade-in w-36 shrink-0 space-y-1">
            {logs.map((f) => (
              <button
                key={f.path}
                onClick={() => selectLog(f.path)}
                className={`w-full truncate rounded px-2 py-1 text-left text-xs transition-colors duration-150 ${
                  selected === f.path
                    ? "bg-primary text-primary-foreground"
                    : "hover:bg-muted"
                }`}
              >
                {f.path}
              </button>
            ))}
          </div>
        )}
        {/* 日志内容 */}
        <ScrollArea
          viewportRef={scrollRef}
          className="min-h-0 min-w-0 flex-1 overflow-hidden font-mono text-xs"
        >
          <pre className="whitespace-pre-wrap break-words">
            {liveMode ? streamText || "等待日志…" : content || "暂无日志"}
          </pre>
        </ScrollArea>
      </div>
    </div>
  );
}
