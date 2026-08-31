// dsh-launcher 主界面：dark 主题
// 布局：左侧主区两列（状态+Web GUI/工具链、版本管理/设置）+ 右侧日志边栏（可展开收起）
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import StatusCard from "@/components/StatusCard";
import ToolchainPanel from "@/components/ToolchainPanel";
import LogPanel from "@/components/LogPanel";
import VersionPanel from "@/components/VersionPanel";
import SettingsPanel from "@/components/SettingsPanel";
import { Toaster } from "@/components/ui/sonner";
import { Separator } from "@/components/ui/separator";
import { Button } from "@/components/ui/button";
import { PanelRightOpen } from "lucide-react";

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

  // 日志边栏展开/收起
  const [logOpen, setLogOpen] = useState(true);

  return (
    <div className="min-h-screen bg-background text-foreground">
      {/* 1600 窗口：整体 flex 行，日志边栏贯穿右侧全高（含标题栏区域） */}
      <div className="mx-auto flex max-w-[1560px] p-4">
        {/* 主区（头部 + 两列内容） */}
        <div className="min-w-0 flex-1 pr-6">
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

          {/* 主区内容：两列（状态/工具链 + 版本/设置） */}
          <div className="grid grid-cols-1 items-stretch gap-6 lg:grid-cols-2">
            {/* 左列 */}
            <div className="flex min-w-0 flex-col">
              <StatusCard />
              <Separator className="my-4" />
              <ToolchainPanel />
            </div>

            {/* 右列 */}
            <div className="flex min-w-0 flex-col">
              <VersionPanel />
              <Separator className="my-4" />
              <SettingsPanel />
            </div>
          </div>
        </div>

        {/* 日志边栏：贯穿右侧全高；展开为子窗口外观，收起为贯穿窄条 */}
        {logOpen ? (
          <div className="-my-4 -mr-4 w-[340px] shrink-0 p-4 pl-0">
            <LogPanel
              className="h-full"
              onClose={() => setLogOpen(false)}
            />
          </div>
        ) : (
          <div className="-my-4 -mr-4 flex shrink-0 flex-col items-center gap-3 border-l bg-muted/20 py-4">
            {/* 贯穿窄条：顶部展开按钮 + 竖排标签 */}
            <Button
              variant="outline"
              size="icon"
              onClick={() => setLogOpen(true)}
              title="展开日志"
            >
              <PanelRightOpen />
            </Button>
            <span className="rotate-90 text-[11px] whitespace-nowrap text-muted-foreground">
              日志
            </span>
          </div>
        )}
      </div>
      <Toaster theme="dark" position="top-right" />
    </div>
  );
}
