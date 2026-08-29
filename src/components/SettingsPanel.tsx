// 设置面板：镜像源配置 + 滑动开关 + 版本管理入口
import { useCallback, useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import {
  getConfig,
  getInstalledVersion,
  setMirrors,
  setSwitches,
} from "@/lib/tauri";
import { toast } from "sonner";
import { Settings } from "lucide-react";

// 常用镜像下拉预设
const PRESETS = {
  npm: [
    { label: "官方源", value: "" },
    { label: "npmmirror（淘宝）", value: "https://registry.npmmirror.com" },
    { label: "华为云", value: "https://repo.huaweicloud.com/repository/npm/" },
  ],
  github: [
    { label: "官方", value: "" },
    { label: "ghproxy 加速", value: "https://ghproxy.com" },
  ],
  node: [
    { label: "官方 nodejs.org", value: "" },
    { label: "npmmirror 镜像", value: "https://npmmirror.com/mirrors/node" },
  ],
};

export default function SettingsPanel() {
  const [installedVersion, setInstalledVersion] = useState<string | null>(null);

  // 表单状态
  const [npmRegistry, setNpmRegistry] = useState("");
  const [githubMirror, setGithubMirror] = useState("");
  const [nodeMirror, setNodeMirror] = useState("");

  // 滑动开关
  const [switches, setSwitchesState] = useState({
    closeExits: false,
    minimizeToTray: true,
    keepDshOnExit: false,
    keepDshHomeOnUninstall: true,
    autoStartDsh: false,
    autoOpenBrowser: true,
  });

  const refresh = useCallback(async () => {
    try {
      const c = await getConfig();
      setNpmRegistry(c.npmRegistry);
      setGithubMirror(c.githubMirror);
      setNodeMirror(c.nodeMirror);
      setSwitchesState({
        closeExits: c.closeExits,
        minimizeToTray: c.minimizeToTray,
        keepDshOnExit: c.keepDshOnExit,
        keepDshHomeOnUninstall: c.keepDshHomeOnUninstall,
        autoStartDsh: c.autoStartDsh,
        autoOpenBrowser: c.autoOpenBrowser,
      });
    } catch (e) {
      console.error("读取配置失败", e);
    }
    try {
      setInstalledVersion(await getInstalledVersion());
    } catch {
      setInstalledVersion(null);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  async function saveMirrors() {
    try {
      await setMirrors({ npmRegistry, githubMirror, nodeMirror });
      toast.success("镜像源已保存（仅影响后续安装/更新）");
    } catch (e) {
      toast.error(`保存失败: ${e}`);
    }
  }

  async function toggleSwitch(key: keyof typeof switches, value: boolean) {
    const next = { ...switches, [key]: value };
    setSwitchesState(next);
    try {
      await setSwitches(next);
      toast.success("设置已保存");
    } catch (e) {
      toast.error(`保存失败: ${e}`);
      refresh();
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Settings className="size-4" />
          设置
          <Badge variant="outline">
            {installedVersion ? `dsh ${installedVersion}` : "dsh 未安装"}
          </Badge>
        </CardTitle>
        <CardDescription>镜像源、滑动开关与运行行为</CardDescription>
      </CardHeader>
      <CardContent className="space-y-5">
        {/* 镜像源 */}
        <div className="space-y-3">
          <h3 className="text-sm font-medium">镜像源</h3>
          <div className="grid gap-2">
            <Label htmlFor="npm">npm registry</Label>
            <div className="flex gap-2">
              <Input
                id="npm"
                value={npmRegistry}
                onChange={(e) => setNpmRegistry(e.target.value)}
                placeholder="https://registry.npmjs.org"
              />
              <select
                className="w-40 rounded-md border bg-background px-2 text-xs"
                onChange={(e) => setNpmRegistry(e.target.value)}
                value={PRESETS.npm.some((p) => p.value === npmRegistry) ? npmRegistry : ""}
              >
                {PRESETS.npm.map((p) => (
                  <option key={p.label} value={p.value}>
                    {p.label}
                  </option>
                ))}
              </select>
            </div>

            <Label htmlFor="gh">GitHub 加速</Label>
            <div className="flex gap-2">
              <Input
                id="gh"
                value={githubMirror}
                onChange={(e) => setGithubMirror(e.target.value)}
                placeholder="https://ghproxy.com"
              />
              <select
                className="w-40 rounded-md border bg-background px-2 text-xs"
                onChange={(e) => setGithubMirror(e.target.value)}
                value={PRESETS.github.some((p) => p.value === githubMirror) ? githubMirror : ""}
              >
                {PRESETS.github.map((p) => (
                  <option key={p.label} value={p.value}>
                    {p.label}
                  </option>
                ))}
              </select>
            </div>

            <Label htmlFor="node">Node 二进制镜像</Label>
            <div className="flex gap-2">
              <Input
                id="node"
                value={nodeMirror}
                onChange={(e) => setNodeMirror(e.target.value)}
                placeholder="https://nodejs.org/dist"
              />
              <select
                className="w-40 rounded-md border bg-background px-2 text-xs"
                onChange={(e) => setNodeMirror(e.target.value)}
                value={PRESETS.node.some((p) => p.value === nodeMirror) ? nodeMirror : ""}
              >
                {PRESETS.node.map((p) => (
                  <option key={p.label} value={p.value}>
                    {p.label}
                  </option>
                ))}
              </select>
            </div>
            <Button variant="secondary" size="sm" onClick={saveMirrors}>
              保存镜像源
            </Button>
          </div>
        </div>

        <Separator />

        {/* 滑动开关 */}
        <div className="space-y-3">
          <h3 className="text-sm font-medium">窗口与运行行为</h3>
          {(
            [
              ["closeExits", "关闭按钮直接退出（含 dsh）", "开启后点关闭会停止 dsh 并退出程序"],
              ["minimizeToTray", "最小化到托盘", "最小化时隐藏到系统托盘而非任务栏"],
              ["keepDshOnExit", "退出时驻留 dsh", "托盘退出时保留 dsh 在后台运行"],
              ["keepDshHomeOnUninstall", "卸载保留 DSH_HOME", "卸载 dsh 时保留用户数据目录"],
              ["autoStartDsh", "启动时自动启动 dsh", "打开启动器时自动拉起 dsh web"],
              ["autoOpenBrowser", "启动时自动打开浏览器", "dsh 启动后自动打开 Web 界面"],
            ] as const
          ).map(([key, label, desc]) => (
            <div key={key} className="flex items-center justify-between">
              <div>
                <div className="text-sm font-medium">{label}</div>
                <div className="text-xs text-muted-foreground">{desc}</div>
              </div>
              <Switch
                checked={switches[key]}
                onCheckedChange={(v) => toggleSwitch(key, v === true)}
              />
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  );
}
