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
 * 数字段不同 → 数字大的在前；数字段相同 → 按 semver 预发布规则比较
 * （alpha/beta/rc 逐段数值化：rc.9 < rc.10；数字段 < 字母段；无预发布 > 有预发布）。
 * v0.4.13（审计修复 I8）：此前平局用 localeCompare 字典序，rc.9 会被排到 rc.10 之前。
 */
export function versionSortDesc(a: string, b: string): number {
  const pa = parseVersionSegments(a);
  const pb = parseVersionSegments(b);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const na = pa[i] ?? 0;
    const nb = pb[i] ?? 0;
    if (na !== nb) return nb - na;
  }
  return -comparePreAsc(preReleaseOf(a), preReleaseOf(b));
}

/** 预发布段：`-rc.2` / `-alpha.1` 拆为标识符数组；无预发布返回 null（正式版） */
function preReleaseOf(version: string): string[] | null {
  const v = version.replace(/^dsh-/i, "").replace(/^v/i, "");
  const m = v.match(/-([0-9A-Za-z.]+)$/);
  return m ? m[1].split(".") : null;
}

type PreTok = { kind: "num"; val: number } | { kind: "str"; val: string };

function tokenizePre(seg: string): PreTok {
  const n = Number(seg);
  return Number.isNaN(n) ? { kind: "str", val: seg } : { kind: "num", val: n };
}

/** 预发布升序比较（<0 = a 更旧/在前）。semver：数字段 < 字母段；前缀相同更长者更新 */
function comparePreAsc(a: string[] | null, b: string[] | null): number {
  // 正式版（null）比任何预发布都新：升序时正式版在最后 → a 正式版时 a>b → 返回正
  if (a === null && b === null) return 0;
  if (a === null) return 1;
  if (b === null) return -1;
  const n = Math.max(a.length, b.length);
  for (let i = 0; i < n; i++) {
    const x = i < a.length ? tokenizePre(a[i]) : null;
    const y = i < b.length ? tokenizePre(b[i]) : null;
    if (x === null) return -1; // a 更短且前缀相同 → a 更旧
    if (y === null) return 1;
    const cmp =
      x.kind === "num" && y.kind === "num"
        ? x.val - y.val
        : x.kind === "num"
          ? -1 // 数字段 < 字母段
          : y.kind === "num"
            ? 1
            : x.val.localeCompare(y.val);
    if (cmp !== 0) return cmp;
  }
  return a.length - b.length;
}
