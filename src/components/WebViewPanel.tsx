// Web GUI 面板：内嵌 deepseek-harness 原生 Web UI
// 使用 Tauri 2 WebviewWindow 创建指向 http://127.0.0.1:<port> 的内嵌窗口
// 端口从配置读取（默认 3080），与启动端口联动
import { useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { getConfig } from "@/lib/tauri";
import { toast } from "sonner";
import { ExternalLink, MonitorUp } from "lucide-react";

export default function WebViewPanel() {
  const [port, setPort] = useState(3080);

  // 读取配置端口（兜底 3080）
  useEffect(() => {
    getConfig()
      .then((c) => {
        if (c.port >= 1 && c.port <= 65535) setPort(c.port);
      })
      .catch(() => {});
  }, []);

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
          <MonitorUp className="size-4" />
          Web GUI
          <Badge variant="outline">端口 {port}</Badge>
        </CardTitle>
        <CardDescription>内嵌 deepseek-harness 原生 Web 界面</CardDescription>
      </CardHeader>
      <CardContent className="flex gap-2">
        <Button onClick={openWebGui}>
          <MonitorUp /> 内嵌打开
        </Button>
        <Button variant="outline" onClick={openExternal}>
          <ExternalLink /> 外部浏览器
        </Button>
      </CardContent>
    </Card>
  );
}
