/**
 * 校验：src 里引用的每个 var(--x) 都能在某处被定义
 * （main.css :root / body 规则 或 theme store 的 setProperty / 内联 :style 变量）。
 * 用于 codemod 后兜底，防止替换出未定义变量导致样式静默失效。
 */
const fs = require("fs");
const path = require("path");

const ROOT = path.join(__dirname, "..", "src");

function walk(dir, acc = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p, acc);
    else if (/\.(vue|css|ts)$/.test(e.name)) acc.push(p);
  }
  return acc;
}

const files = walk(ROOT);
const used = new Map(); // name -> [file]
const defined = new Set();

for (const f of files) {
  const code = fs.readFileSync(f, "utf8");
  const rel = path.relative(path.join(__dirname, ".."), f);

  // 使用点：var(--x)
  for (const m of code.matchAll(/var\(\s*(--[a-zA-Z0-9-]+)/g)) {
    if (!used.has(m[1])) used.set(m[1], new Set());
    used.get(m[1]).add(rel);
  }
  // 定义点 A：CSS 声明 `--x:`
  for (const m of code.matchAll(/(^|[\s{;])(--[a-zA-Z0-9-]+)\s*:/g)) defined.add(m[2]);
  // 定义点 B：JS setProperty("--x", ...) / " '--x': value" / property: "--x"（对象字面量）
  for (const m of code.matchAll(/setProperty\(\s*["'`](--[a-zA-Z0-9-]+)/g)) defined.add(m[1]);
  for (const m of code.matchAll(/["'`](--[a-zA-Z0-9-]+)["'`]\s*:/g)) defined.add(m[1]);
}

// 兜底白名单：浏览器原生或他处注入
const KNOWN = new Set([" --x"]);

const missing = [];
for (const [name, where] of used) {
  if (defined.has(name)) continue;
  if (KNOWN.has(name)) continue;
  missing.push(`${name}  ← ${[...where].slice(0, 3).join(", ")}`);
}

console.log(`引用变量 ${used.size} 个，已定义 ${defined.size} 个`);
if (missing.length) {
  console.log(`\n未定义（${missing.length}）:`);
  console.log(missing.sort().join("\n"));
} else {
  console.log("全部已定义 ✓");
}
