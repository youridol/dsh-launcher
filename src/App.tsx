// dsh-launcher 主界面：dark 主题
// 布局：左侧 状态卡片 + 工具链；右侧 Web GUI + 日志
import { useEffect } from "react";
import StatusCard from "@/components/StatusCard";
import ToolchainPanel from "@/components/ToolchainPanel";
import WebViewPanel from "@/components/WebViewPanel";
import LogPanel from "@/components/LogPanel";
import { Toaster } from "@/components/ui/sonner";

export default function App() {
  // 强制黑暗主题
  useEffect(() => {
    document.documentElement.classList.add("dark");
  }, []);

  return (
    <div className="min-h-screen bg-background text-foreground">
      <div className="mx-auto max-w-6xl space-y-4 p-4">
        <header className="flex items-center justify-between">
          <div>
            <h1 className="text-xl font-semibold">dsh-launcher</h1>
            <p className="text-sm text-muted-foreground">
              deepseek-harness 启动器与运行环境管理器
            </p>
          </div>
          <div className="text-xs text-muted-foreground">v0.1.0</div>
        </header>

        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          <div className="space-y-4">
            <StatusCard />
            <ToolchainPanel />
          </div>
          <div className="space-y-4">
            <WebViewPanel />
            <LogPanel />
          </div>
        </div>
      </div>
      <Toaster theme="dark" position="top-right" />
    </div>
  );
}
