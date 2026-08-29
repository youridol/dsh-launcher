// dsh-launcher 主界面：dark 主题
// 单页布局：所有功能整合在一页，用分割线分隔区块
// 区块顺序：状态控制 → 工具链 → Web GUI → 版本管理 → 设置 → 日志
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
      <div className="mx-auto max-w-5xl space-y-0 p-4">
        {/* 头部 */}
        <header className="flex items-center justify-between pb-4">
          <div>
            <h1 className="text-xl font-semibold">dsh-launcher</h1>
            <p className="text-sm text-muted-foreground">
              deepseek-harness 启动器与运行环境管理器
            </p>
          </div>
          <div className="text-xs text-muted-foreground">v0.1.4</div>
        </header>

        {/* 区块 1：dsh 状态与生命周期控制 */}
        <StatusCard />

        <Separator className="my-5" />

        {/* 区块 2：工具链与运行环境 */}
        <ToolchainPanel />

        <Separator className="my-5" />

        {/* 区块 3：Web GUI */}
        <WebViewPanel />

        <Separator className="my-5" />

        {/* 区块 4：版本管理 */}
        <VersionPanel />

        <Separator className="my-5" />

        {/* 区块 5：设置 */}
        <SettingsPanel />

        <Separator className="my-5" />

        {/* 区块 6：日志 */}
        <LogPanel />
      </div>
      <Toaster theme="dark" position="top-right" />
    </div>
  );
}
