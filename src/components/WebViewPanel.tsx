// Web GUI 面板：内嵌 deepseek-harness 原生 Web UI
// 使用 Tauri 2 WebviewWindow 创建指向 http://127.0.0.1:<port> 的内嵌窗口
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { ExternalLink, MonitorUp } from "lucide-react";

export default function WebViewPanel() {
  async function openWebGui() {
    // 读取端口：从 Rust 配置（当前简化：固定 3080，后续从后端拉取）
    const port = 3080;
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
    }
  }

  function openExternal() {
    const port = 3080;
    window.open(`http://127.0.0.1:${port}`, "_blank");
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <MonitorUp className="size-4" />
          Web GUI
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
