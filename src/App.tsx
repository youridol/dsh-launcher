// dsh-launcher 主界面：dark 主题
// 布局：三栏（左侧栏 状态+工具链 / 中央 版本管理 / 右侧 日志），骨架见 AppShell
// 设置面板：标题栏"设置"按钮 → 弹出子窗口（Dialog，状态在本组件管理）
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import AppShell from "@/components/AppShell";
import SettingsPanel from "@/components/SettingsPanel";
import { Toaster } from "@/components/ui/sonner";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

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

  // 设置弹出子窗口开关
  const [settingsOpen, setSettingsOpen] = useState(false);

  return (
    <>
      {/* 三栏布局骨架（布局/拖拽/响应式/持久化全在 AppShell） */}
      <AppShell version={version} onOpenSettings={() => setSettingsOpen(true)} />
      {/* 设置弹出子窗口（Dialog） */}
      <Dialog open={settingsOpen} onOpenChange={setSettingsOpen}>
        <DialogContent className="max-h-[85vh] w-full max-w-lg overflow-y-auto">
          <DialogHeader>
            <DialogTitle>设置</DialogTitle>
          </DialogHeader>
          <SettingsPanel embedded />
        </DialogContent>
      </Dialog>

      <Toaster theme="dark" position="top-right" />
    </>
  );
}