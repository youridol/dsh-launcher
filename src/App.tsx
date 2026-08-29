// dsh-launcher 主界面：dark 主题
// 布局：Tabs（总览 / 版本管理 / 设置）
import { useEffect } from "react";
import StatusCard from "@/components/StatusCard";
import ToolchainPanel from "@/components/ToolchainPanel";
import WebViewPanel from "@/components/WebViewPanel";
import LogPanel from "@/components/LogPanel";
import VersionPanel from "@/components/VersionPanel";
import SettingsPanel from "@/components/SettingsPanel";
import { Toaster } from "@/components/ui/sonner";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";

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

        {/* 非受控 Tabs（base-ui 官方推荐模式，避免受控 value 状态竞争导致切换无响应） */}
        <Tabs defaultValue="overview">
          <TabsList>
            <TabsTrigger value="overview">总览</TabsTrigger>
            <TabsTrigger value="versions">版本管理</TabsTrigger>
            <TabsTrigger value="settings">设置</TabsTrigger>
          </TabsList>

          <TabsContent value="overview" className="mt-4">
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
          </TabsContent>

          <TabsContent value="versions" className="mt-4">
            <VersionPanel />
          </TabsContent>

          <TabsContent value="settings" className="mt-4">
            <SettingsPanel />
          </TabsContent>
        </Tabs>
      </div>
      <Toaster theme="dark" position="top-right" />
    </div>
  );
}
