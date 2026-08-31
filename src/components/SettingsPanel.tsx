// 设置面板：镜像源配置 + 滑动开关 + 版本管理入口
import { useCallback, useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import {
  getConfig,
  getInstalledVersion,
  setGithubToken,
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

export default function SettingsPanel({
  embedded = false,
}: {
  // true = 在弹出子窗口/对话框内使用（无 Card 外壳与重复标题）
  embedded?: boolean;
}) {
  const [installedVersion, setInstalledVersion] = useState<string | null>(null);

  // 表单状态
  const [npmRegistry, setNpmRegistry] = useState("");
  const [githubMirror, setGithubMirror] = useState("");
  const [nodeMirror, setNodeMirror] = useState("");
  // GitHub Token（防限流）
  const [githubToken, setGithubTokenState] = useState("");

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
      setGithubTokenState(c.githubToken);
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

  async function saveToken() {
    try {
      await setGithubToken(githubToken);
      toast.success("GitHub Token 已保存");
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
    <div className="space-y-4">
      {!embedded && (
        <CardHeader className="px-0 pt-0">
          <CardTitle className="flex items-center gap-2">
            <Settings className="size-4" />
            设置
            <Badge variant="outline">
              {installedVersion ? `dsh ${installedVersion}` : "dsh 未安装"}
            </Badge>
          </CardTitle>
          <CardDescription>镜像源与运行行为</CardDescription>
        </CardHeader>
      )}
      {embedded && installedVersion && (
        <div className="text-xs text-muted-foreground">
          dsh {installedVersion}
        </div>
      )}
        {/* 镜像源 */}
        <div className="space-y-2">
          <h3 className="text-sm font-medium">镜像源</h3>
          <div className="grid gap-2">
            <Label htmlFor="npm" className="text-xs">npm registry</Label>
            <div className="flex gap-1.5">
              <Input
                id="npm"
                value={npmRegistry}
                onChange={(e) => setNpmRegistry(e.target.value)}
                placeholder="https://registry.npmjs.org"
                className="text-xs"
              />
              <select
                className="w-32 shrink-0 rounded-md border bg-background px-1.5 text-xs"
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

            <Label htmlFor="gh" className="text-xs">GitHub 加速</Label>
            <div className="flex gap-1.5">
              <Input
                id="gh"
                value={githubMirror}
                onChange={(e) => setGithubMirror(e.target.value)}
                placeholder="https://ghproxy.com"
                className="text-xs"
              />
              <select
                className="w-32 shrink-0 rounded-md border bg-background px-1.5 text-xs"
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

            <Label htmlFor="node" className="text-xs">Node 镜像</Label>
            <div className="flex gap-1.5">
              <Input
                id="node"
                value={nodeMirror}
                onChange={(e) => setNodeMirror(e.target.value)}
                placeholder="https://nodejs.org/dist"
                className="text-xs"
              />
              <select
                className="w-32 shrink-0 rounded-md border bg-background px-1.5 text-xs"
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

        {/* GitHub Token：防 API 限流 / git 认证增强 */}
        <div className="space-y-2">
          <h3 className="text-sm font-medium">GitHub Token</h3>
          <div className="grid gap-1.5">
            <Input
              type="password"
              value={githubToken}
              onChange={(e) => setGithubTokenState(e.target.value)}
              placeholder="ghp_xxx（Personal Access Token，可选）"
              className="text-xs"
            />
            <p className="text-[11px] text-muted-foreground">
              用于 git 认证增强，避免 GitHub API/克隆限流；留空为匿名访问
            </p>
            <Button variant="secondary" size="sm" onClick={saveToken}>
              保存 Token
            </Button>
          </div>
        </div>

        <Separator />

        {/* 滑动开关：两列排布，描述简化 */}
        <div className="space-y-2">
          <h3 className="text-sm font-medium">窗口与运行行为</h3>
          <div className="grid grid-cols-2 gap-x-4 gap-y-2">
            {(
              [
                ["closeExits", "关闭直接退出"],
                ["minimizeToTray", "最小化到托盘"],
                ["keepDshOnExit", "退出驻留 dsh"],
                ["keepDshHomeOnUninstall", "卸载保留数据"],
                ["autoStartDsh", "启动时自动启动"],
                ["autoOpenBrowser", "启动时打开浏览器"],
              ] as const
            ).map(([key, label]) => (
              <div key={key} className="flex items-center justify-between gap-2">
                <span className="text-xs">{label}</span>
                <Switch
                  size="sm"
                  checked={switches[key]}
                  onCheckedChange={(v) => toggleSwitch(key, v === true)}
                />
              </div>
            ))}
          </div>
        </div>
    </div>
  );
}
