// 日志面板：全域日志流式展示（dsh + 启动器）
// 实时流：Tauri event `log://line`（Rust logging.rs emit）
// 兜底：文件列表 + 定期读取
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { listLogs, readLog, type LogFile } from "@/lib/tauri";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { FileText, Download, RefreshCw } from "lucide-react";

// 日志行（对应 Rust LogLine）
interface LogLine {
  timestamp: string;
  source: string;
  level: string;
  message: string;
}

// 实时流最大缓冲行数
const MAX_STREAM_LINES = 2000;

export default function LogPanel() {
  const [logs, setLogs] = useState<LogFile[]>([]);
  const [content, setContent] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  // 实时流行（追加模式）
  const [streamLines, setStreamLines] = useState<LogLine[]>([]);
  // 是否为"实时流"视图
  const [liveMode, setLiveMode] = useState(true);
  // 内容容器引用（自动滚动）
  const scrollRef = useRef<HTMLDivElement>(null);

  // 订阅 Rust 日志事件（实时流）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listen<LogLine>("log://line", (event) => {
          setStreamLines((prev) => {
            const next = [...prev, event.payload];
            // 超过缓冲上限则丢弃最旧的
            if (next.length > MAX_STREAM_LINES) {
              return next.slice(next.length - MAX_STREAM_LINES);
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

  // 自动滚动到底部
  useEffect(() => {
    if (liveMode && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
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
    refreshFiles();
    // 文件列表定期刷新（30 秒，实时流为主）
    const timer = setInterval(refreshFiles, 30000);
    return () => clearInterval(timer);
  }, [refreshFiles]);

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
    <Card className="flex h-full flex-col">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <FileText className="size-4" />
          日志
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
          <Button variant="ghost" size="icon-sm" onClick={refreshFiles} title="刷新文件列表">
            <RefreshCw />
          </Button>
          <Button variant="ghost" size="icon-sm" onClick={exportLog} title="导出日志">
            <Download />
          </Button>
        </CardTitle>
        <CardDescription>
          全域日志（dsh + 启动器）{liveMode ? "· 实时流" : "· 文件"}
        </CardDescription>
      </CardHeader>
      <CardContent className="flex h-full gap-3">
        {/* 日志文件列表（非实时流时显示） */}
        {!liveMode && (
          <div className="w-40 shrink-0 space-y-1">
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
        <ScrollArea className="flex-1 rounded-md border bg-muted/30 p-2 font-mono text-xs">
          <div
            ref={scrollRef}
            className="h-full overflow-auto"
          >
            <pre className="whitespace-pre-wrap">
              {liveMode
                ? streamText || "等待日志…"
                : content || "暂无日志"}
            </pre>
          </div>
        </ScrollArea>
      </CardContent>
    </Card>
  );
}
