// 日志面板：全域日志流式展示（dsh + 启动器）
import { useCallback, useEffect, useState } from "react";
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
import { toast } from "sonner";
import { FileText, Download, RefreshCw } from "lucide-react";

export default function LogPanel() {
  const [logs, setLogs] = useState<LogFile[]>([]);
  const [content, setContent] = useState("");
  const [selected, setSelected] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const files = await listLogs();
      setLogs(files);
      if (files.length > 0 && !selected) {
        const first = files[0];
        setSelected(first.path);
        setContent(await readLog(first.path));
      }
    } catch (e) {
      console.error("读取日志失败", e);
    }
  }, [selected]);

  useEffect(() => {
    refresh();
    // 定期刷新（5 秒）
    const timer = setInterval(refresh, 5000);
    return () => clearInterval(timer);
  }, [refresh]);

  async function selectLog(path: string) {
    setSelected(path);
    try {
      setContent(await readLog(path));
    } catch (e) {
      toast.error(`读取日志失败: ${e}`);
    }
  }

  function exportLog() {
    // 触发下载（浏览器环境）
    const blob = new Blob([content], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = selected?.replace("/", "-") ?? "dsh.log";
    a.click();
    URL.revokeObjectURL(url);
  }

  return (
    <Card className="flex h-full flex-col">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <FileText className="size-4" />
          日志
          <Button variant="ghost" size="icon-sm" onClick={refresh} title="刷新">
            <RefreshCw />
          </Button>
          <Button variant="ghost" size="icon-sm" onClick={exportLog} title="导出日志">
            <Download />
          </Button>
        </CardTitle>
        <CardDescription>全域日志（dsh + 启动器）</CardDescription>
      </CardHeader>
      <CardContent className="flex h-full gap-3">
        {/* 日志文件列表 */}
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
        {/* 日志内容 */}
        <ScrollArea className="flex-1 rounded-md border bg-muted/30 p-2 font-mono text-xs">
          <pre className="whitespace-pre-wrap">{content || "暂无日志"}</pre>
        </ScrollArea>
      </CardContent>
    </Card>
  );
}
