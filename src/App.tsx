// dsh-launcher 主界面：dark 主题
// 三列布局：左（状态+Web GUI/工具链）、中（版本管理/设置）、右（日志独立列）
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import StatusCard from "@/components/StatusCard";
import ToolchainPanel from "@/components/ToolchainPanel";
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

  // 应用版本号（来自 tauri.conf.json，构建时注入）
  const [version, setVersion] = useState("");
  useEffect(() => {
    getVersion()
      .then((v) => setVersion(`v${v}`))
      .catch(() => setVersion(""));
  }, []);

  return (
    <div className="min-h-screen bg-background text-foreground">
      {/* 1600 窗口：容器撑满不超边界（max-w 留少量边距） */}
      <div className="mx-auto max-w-[1560px] p-4">
        {/* 头部 */}
        <header className="flex items-center justify-between pb-4">
          <div>
            <h1 className="text-lg font-semibold">dsh-launcher</h1>
            <p className="text-xs text-muted-foreground">
              deepseek-harness 启动器
            </p>
          </div>
          <div className="text-xs text-muted-foreground">{version}</div>
        </header>

        {/* 三列布局：左（状态/工具链）中（版本/设置）右（日志独立）
            列等高对齐；日志列固定视口高度内部滚动，卡片不溢出窗口 */}
        <div className="grid grid-cols-1 items-stretch gap-6 lg:grid-cols-3">
          {/* 左列 */}
          <div className="flex min-w-0 flex-col">
            <StatusCard />
            <Separator className="my-4" />
            <ToolchainPanel />
          </div>

          {/* 中列 */}
          <div className="flex min-w-0 flex-col">
            <VersionPanel />
            <Separator className="my-4" />
            <SettingsPanel />
          </div>

          {/* 右列：日志独立第三列，占满可用高度（视口高 - 头部/边距），内部滚动 */}
          <div className="flex min-w-0 flex-col">
            <LogPanel className="h-[calc(100vh-130px)] min-h-[360px] flex-1" />
          </div>
        </div>
      </div>
      <Toaster theme="dark" position="top-right" />
    </div>
  );
}
