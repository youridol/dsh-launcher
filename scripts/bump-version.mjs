// 版本递增脚本：master 每次提交自动迭代 PATCH 版本（0.3.x → 0.3.(x+1)）
// 同步 4 处版本号：package.json / src-tauri/Cargo.toml / src-tauri/tauri.conf.json / package-lock.json
// 用法：node scripts/bump-version.mjs [--major|--minor|--patch]
// 默认 patch；--minor 升次版本号（新功能）；--major 升主版本号（破坏性变更）。
// 注：是否已发布（对应 tag 已存在）由调用方（发布工作流）判断；本脚本只做版本递增与四文件同步。

import { readFileSync, writeFileSync } from "node:fs";

const flag = process.argv[2] ?? "--patch";
if (!["--major", "--minor", "--patch"].includes(flag)) {
  console.error(`非法参数: ${flag}（支持 --major / --minor / --patch）`);
  process.exit(1);
}

// 从 package.json 读取当前版本（唯一权威来源）
const pkg = JSON.parse(readFileSync("package.json", "utf8"));
const [major, minor, patch] = pkg.version.split(".").map(Number);
if (![major, minor, patch].every((n) => Number.isInteger(n) && n >= 0)) {
  console.error(`无法解析版本号: ${pkg.version}`);
  process.exit(1);
}

let next = { major, minor, patch };
if (flag === "--major") {
  next = { major: major + 1, minor: 0, patch: 0 };
} else if (flag === "--minor") {
  next = { major, minor: minor + 1, patch: 0 };
} else {
  next = { major, minor, patch: patch + 1 };
}
const nextVersion = `${next.major}.${next.minor}.${next.patch}`;

// 读取各文件中的当前版本
function readVersionIn(file, pattern) {
  const content = readFileSync(file, "utf8");
  const m = content.match(pattern);
  return m ? m[1] : null;
}

const cargoVersion = readVersionIn("src-tauri/Cargo.toml", /^version = "([^"]+)"/m);
const tauriVersion = readVersionIn("src-tauri/tauri.conf.json", /"version": "([^"]+)"/);
// package-lock.json：用 JSON 解析根包 version（正则易受字段顺序影响）
const lockJson = JSON.parse(readFileSync("package-lock.json", "utf8"));
const lockRootVersion = lockJson.packages?.[""]?.version ?? null;
// Cargo.lock：dsh-launcher 包版本
const cargoLockVersion = readVersionIn(
  "src-tauri/Cargo.lock",
  /name = "dsh-launcher"\nversion = "([^"]+)"/,
);

// 一致性保护：五处版本必须一致（package.json 为权威），不一致则报错阻止
const allVersions = [pkg.version, cargoVersion, tauriVersion, lockRootVersion, cargoLockVersion];
if (new Set(allVersions).size !== 1) {
  console.error(
    `版本号不同步，请先手动对齐（package.json=${pkg.version}, Cargo.toml=${cargoVersion}, tauri.conf.json=${tauriVersion}, package-lock.json=${lockRootVersion}, Cargo.lock=${cargoLockVersion}）`,
  );
  process.exit(1);
}

// 更新 package.json
pkg.version = nextVersion;
writeFileSync("package.json", JSON.stringify(pkg, null, 2) + "\n");

// Cargo.toml：version = "x.y.z"
const cargo = readFileSync("src-tauri/Cargo.toml", "utf8");
writeFileSync(
  "src-tauri/Cargo.toml",
  cargo.replace(/^version = "[^"]+"/m, `version = "${nextVersion}"`),
);

// tauri.conf.json：version 字段
const tauriConf = readFileSync("src-tauri/tauri.conf.json", "utf8");
writeFileSync(
  "src-tauri/tauri.conf.json",
  tauriConf.replace(/"version": "[^"]+"/, `"version": "${nextVersion}"`),
);

// package-lock.json：根包 version
const lock = JSON.parse(readFileSync("package-lock.json", "utf8"));
if (lock.packages?.[""]) {
  lock.packages[""].version = nextVersion;
}
lock.version = nextVersion;
writeFileSync("package-lock.json", JSON.stringify(lock, null, 2) + "\n");

// Cargo.lock：dsh-launcher 包 version（跟随 Cargo.toml，保持构建一致性）
const cargoLock = readFileSync("src-tauri/Cargo.lock", "utf8");
writeFileSync(
  "src-tauri/Cargo.lock",
  cargoLock.replace(
    /(name = "dsh-launcher"\nversion = ")[^"]+(")/,
    `$1${nextVersion}$2`,
  ),
);

console.log(`版本已递增: ${pkg.version} → ${nextVersion}`);
