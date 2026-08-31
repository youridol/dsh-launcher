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
import { PanelRightClose, PanelRightOpen, FileText } from "lucide-react";

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

        {/* 布局：主区两列 + 右侧日志边栏（可展开收起） */}
        <div className="flex items-stretch gap-6">
          {/* 主区：两列（状态/工具链 + 版本/设置） */}
          <div className="grid flex-1 grid-cols-1 items-stretch gap-6 lg:grid-cols-2">
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

          {/* 日志边栏：展开显示日志卡片，收起显示窄条按钮 */}
          {logOpen ? (
            <div className="flex w-[340px] shrink-0 flex-col">
              <div className="mb-2 flex items-center justify-between">
                <span className="flex items-center gap-1.5 text-sm font-medium">
                  <FileText className="size-4" /> 日志
                </span>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  onClick={() => setLogOpen(false)}
                  title="收起日志"
                >
                  <PanelRightClose />
                </Button>
              </div>
              <LogPanel className="h-[calc(100vh-170px)] min-h-[360px] flex-1" />
            </div>
          ) : (
            <div className="flex shrink-0 flex-col items-center justify-center gap-2">
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
      </div>
      <Toaster theme="dark" position="top-right" />
    </div>
  );
}
