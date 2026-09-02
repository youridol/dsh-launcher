// 日志侧边栏：全域日志流式展示（dsh + 启动器），固定在右侧，无卡片容器
// 实时流：Tauri event `log://line`（Rust logging.rs emit）
// 补流：挂载时读取最新日志文件（含启动早期日志与 dsh 日志）→ 放入实时流，避免丢失
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { listLogs, readLog, type LogFile } from "@/lib/tauri";
import { listen } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { toast } from "sonner";
import { Download, RefreshCw, X } from "lucide-react";

// 日志行（对应 Rust LogLine）
interface LogLine {
  timestamp: string;
  source: string;
  level: string;
  message: string;
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

export default function LogPanel({
  className,
  onClose,
}: {
  className?: string;
  onClose?: () => void;
}) {
  const [logs, setLogs] = useState<LogFile[]>([]);
  const [content, setContent] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  // 实时流行（追加模式）
  const [streamLines, setStreamLines] = useState<LogLine[]>([]);
  // 是否为"实时流"视图
  const [liveMode, setLiveMode] = useState(true);
  // 内容容器引用（自动滚动；指向 ScrollArea 的 viewport，即真正的滚动容器）
  const scrollRef = useRef<HTMLDivElement>(null);
  // 已补流的日志文件（避免重复加载同一文件追加）
  const backfilledRef = useRef<string | null>(null);

  // 订阅 Rust 日志事件（实时流，最新条目在最前）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listen<LogLine>("log://line", (event) => {
          setStreamLines((prev) => {
            // 最新条目插到最前（最新在上）
            const next = [event.payload, ...prev];
            // 超过缓冲上限则丢弃最旧的（末尾）
            if (next.length > MAX_STREAM_LINES) {
              return next.slice(0, MAX_STREAM_LINES);
            }
            return next;
          });
        });
      } catch (e) {
        console.error("订阅日志事件失败", e);
      }
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  // 补流：读取最新日志文件 → 解析为 LogLine（倒序：新在前）放入实时流
  // 覆盖 LogPanel 挂载前产生的日志（如启动早期、主窗口操作、dsh 启动输出）
  const backfillLatestFile = useCallback(async () => {
    try {
      const files = await listLogs();
      const latest = files[0];
      if (!latest || backfilledRef.current === latest.path) return;
      const raw = await readLog(latest.path);
      const lines: LogLine[] = [];
      for (const line of raw.split(/\r?\n/).reverse()) {
        const parsed = parseLogLine(line);
        if (parsed) lines.push(parsed);
      }
      if (lines.length > 0) {
        backfilledRef.current = latest.path;
        setStreamLines((prev) => {
          // 合并去重：跳过与已缓冲相同 timestamp+message 的旧条目
          const seen = new Set(prev.slice(0, 50).map((l) => l.timestamp + l.message));
          const merged = [...prev];
          for (const l of lines) {
            if (!seen.has(l.timestamp + l.message)) merged.push(l);
          }
          return merged.slice(0, MAX_STREAM_LINES);
        });
      }
    } catch (e) {
      console.error("补流日志失败", e);
    }
  }, []);

  // 最新在上 → 自动滚动到顶部（滚动容器是 ScrollArea 的 viewport）
  useEffect(() => {
    if (liveMode && scrollRef.current) {
      scrollRef.current.scrollTop = 0;
    }
  }, [streamLines, liveMode]);

  const refreshFiles = useCallback(async () => {
    try {
      const files = await listLogs();
      setLogs(files);
      if (files.length > 0 && !selected) {
        const first = files[0];
        setSelected(first.path);
        setContent(await readLog(first.path));
      }
    } catch (e) {
      console.error("读取日志列表失败", e);
    }
  }, [selected]);

  useEffect(() => {
    // 首次挂载：先补流最新日志（含启动早期/dsh 日志），再刷新文件列表
    backfillLatestFile().finally(() => refreshFiles());
    // 实时流为主：文件列表定期刷新（30 秒）保持文件视图可用；补流仅首帧一次（避免重复追加）
    const timer = setInterval(() => {
      refreshFiles();
    }, 30000);
    return () => clearInterval(timer);
  }, [backfillLatestFile, refreshFiles]);

  async function selectLog(path: string) {
    setLiveMode(false);
    setSelected(path);
    try {
      setContent(await readLog(path));
    } catch (e) {
      toast.error(`读取日志失败: ${e}`);
    }
  }

  function exportLog() {
    // 实时流模式导出流内容；文件模式导出文件内容
    const data = liveMode
      ? streamLines
          .map((l) => `[${l.timestamp}] [${l.source}] [${l.level}] ${l.message}`)
          .join("\n")
      : content;
    const blob = new Blob([data], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = liveMode
      ? "dsh-live.log"
      : (selected?.replace("/", "-") ?? "dsh.log");
    a.click();
    URL.revokeObjectURL(url);
  }

  const streamText = streamLines
    .map((l) => `[${l.timestamp}] [${l.source}] [${l.level}] ${l.message}`)
    .join("\n");

  return (
    // 子窗口外观：独立边框 + 背景 + 圆角，标题栏与内容区分离
    <div
      className={cn(
        "flex h-full flex-col overflow-hidden",  // 全页展示：去除卡片/子窗口外壳（边框/圆角/阴影/背景）
        className
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
          <Button variant="ghost" size="icon-sm" onClick={refreshFiles} title="刷新文件列表">
            <RefreshCw />
          </Button>
          <Button variant="ghost" size="icon-sm" onClick={exportLog} title="导出日志">
            <Download />
          </Button>
          {onClose && (
            <Button variant="outline" size="sm" onClick={onClose} title="收起日志">
              <X className="size-3.5" /> 收起
            </Button>
          )}
        </div>
      </div>

      {/* 内容区（一体化侧边栏：无背景色、无描边；右侧不留 padding，滚动条紧贴右缘） */}
      <div className="flex min-h-0 flex-1 gap-3 pl-4 pt-1 pb-2">
        {/* 日志文件列表（非实时流时显示） */}
        {!liveMode && (
          <div className="w-36 shrink-0 space-y-1">
            {logs.map((f) => (
              <button
                key={f.path}
                onClick={() => selectLog(f.path)}
                className={`w-full truncate rounded px-2 py-1 text-left text-xs transition-colors ${
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
        {/* 日志输出容器：无背景色无描边（整体侧边栏） */}
        <ScrollArea
          viewportRef={scrollRef}
          className="min-h-0 min-w-0 flex-1 overflow-hidden font-mono text-xs"
        >
          <pre className="whitespace-pre-wrap break-words">
            {liveMode
              ? streamText || "等待日志…"
              : content || "暂无日志"}
          </pre>
        </ScrollArea>
      </div>
    </div>
  );
}
