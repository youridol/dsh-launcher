// 版本号排序工具：NPM / GitHub 通道统一按最新版本置顶

/**
 * 规范化版本字符串为可比较的数字段数组
 * 支持格式：
 * - NPM:  0.1.1-rc.2
 * - GitHub: dsh-v0.1.2-alpha.1（带 dsh- 和 v 前缀）
 * 剥离前缀后按数字段比较。
 */
export function parseVersionSegments(version: string): number[] {
  // 去掉 dsh- / v 前缀（GitHub tag 形如 dsh-v0.1.2-alpha.1）
  let v = version.replace(/^dsh-/i, "").replace(/^v/i, "");
  // 取主版本段（去掉 -rc.2 / -alpha.1 等预发布后缀）
  const main = v.split(/[-+]/)[0];
  return main.split(".").map((seg) => {
    const n = Number(seg);
    return Number.isNaN(n) ? 0 : n;
  });
}

/**
 * 版本降序比较（最新在前）
 * 数字段不同 → 数字大的在前；数字段相同 → 按完整字符串倒序（rc > alpha）
 */
export function versionSortDesc(a: string, b: string): number {
  const pa = parseVersionSegments(a);
  const pb = parseVersionSegments(b);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const na = pa[i] ?? 0;
    const nb = pb[i] ?? 0;
    if (na !== nb) return nb - na;
  }
  return b.localeCompare(a);
}
