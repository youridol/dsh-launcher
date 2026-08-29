// dsh-launcher 主界面：dark 主题
// 两列布局：左列（状态控制 / 工具链 / Web GUI），右列（版本管理 / 设置 / 日志）
import { useEffect } from "react";
import StatusCard from "@/components/StatusCard";
import ToolchainPanel from "@/components/ToolchainPanel";
import WebViewPanel from "@/components/WebViewPanel";
import LogPanel from "@/components/LogPanel";
import VersionPanel from "@/components/VersionPanel";
import SettingsPanel from "@/components/SettingsPanel";
import { Toaster } from "@/components/ui/sonner";
import { Separator } from "@/components/ui/separator";

export default function App() {
  // 强制黑暗主题
  useEffect(() => {
    document.documentElement.classList.add("dark");
  }, []);

  return (
    <div className="min-h-screen bg-background text-foreground">
      <div className="mx-auto max-w-7xl p-4">
        {/* 头部 */}
        <header className="flex items-center justify-between pb-4">
          <div>
            <h1 className="text-xl font-semibold">dsh-launcher</h1>
            <p className="text-sm text-muted-foreground">
              deepseek-harness 启动器与运行环境管理器
            </p>
          </div>
          <div className="text-xs text-muted-foreground">v0.1.5</div>
        </header>

        {/* 两列布局：左列 3 区块 + 右列 3 区块 */}
        <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
          {/* 左列 */}
          <div className="space-y-0">
            <StatusCard />
            <Separator className="my-5" />
            <ToolchainPanel />
            <Separator className="my-5" />
            <WebViewPanel />
          </div>

          {/* 右列 */}
          <div className="space-y-0">
            <VersionPanel />
            <Separator className="my-5" />
            <SettingsPanel />
            <Separator className="my-5" />
            <LogPanel />
          </div>
        </div>
      </div>
      <Toaster theme="dark" position="top-right" />
    </div>
  );
}
