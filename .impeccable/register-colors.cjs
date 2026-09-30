/**
 * FEAT-087 codemod：把组件里硬编码的品牌色/语义色「注册」成 CSS 变量，
 * 使全局饱和度管线（theme store 的 sat()）一处生效。
 *
 * 范围与安全边界：
 *  - 只改 <style> 块（.vue）与 .css 文件 —— JS/TS 里若用色值做 canvas 绘制或
 *    颜色计算，替换成 var() 会坏掉，因此脚本部分一律不碰；
 *  - 跳过 styles 基座（main.css）与 utils/stores（那里是变量定义与饱和度管线本身），
 *    避免自引用（--color-primary: var(--color-primary)）。
 *
 * 用法：node .impeccable/register-colors.cjs [--apply]
 *   不带 --apply 只出报告（dry run）。
 */
const fs = require("fs");
const path = require("path");

const ROOT = path.join(__dirname, "..", "src");
const SKIP_FILES = new Set([
  path.join(ROOT, "assets", "main.css"), // 变量定义层：保留字面量基值
  path.join(ROOT, "utils", "color.ts"),
  path.join(ROOT, "utils", "prefs.ts"),
  path.join(ROOT, "utils", "presets.ts"),
  path.join(ROOT, "stores", "theme.ts"),
]);

/** 字面量 → 变量 映射（顺序有意义：长的先替换） */
const HEX_MAP = [
  ["#396cd8", "var(--color-primary)"],
  ["#396CD8", "var(--color-primary)"],
  ["#2f5cc2", "var(--color-primary-hover)"],
  ["#2F5CC2", "var(--color-primary-hover)"],
  ["#d13438", "var(--color-danger)"],
  ["#D13438", "var(--color-danger)"],
  ["#e5484d", "var(--color-danger)"],
  ["#E5484D", "var(--color-danger)"],
  ["#b92e33", "var(--color-danger-hover)"],
  ["#16a34a", "var(--color-ok-vivid)"],
  ["#16A34A", "var(--color-ok-vivid)"],
  // 深色档绿/琥珀：这些是「实底 + 白字」或深底文字的固定值，
  // 不能映射到模式双值的 --color-ok/--color-warn（深色模式会提亮，白字反而失比）
  ["#15803d", "var(--color-ok-solid)"],
  ["#15803D", "var(--color-ok-solid)"],
  ["#b45309", "var(--color-warn-solid)"],
  ["#B45309", "var(--color-warn-solid)"],
  // 深底上的语义文字色（提亮档）：直接作文字色的那几个值
  ["#e03131", "var(--color-danger-text)"],
  ["#E03131", "var(--color-danger-text)"],
  ["#d97706", "var(--color-warn-text)"],
  ["#e8a03c", "var(--color-warn-text)"],
  ["#2f9e44", "var(--color-ok-text)"],
  ["#6ed27a", "var(--color-ok-text)"],
  ["#5bc46a", "var(--color-ok-text)"],
  // 玻璃卡语境链接浅蓝（= body.theme-dark --color-link）
  ["#8ab4ff", "var(--color-link)"],
  ["#8AB4FF", "var(--color-link)"],
  ["#93b4f5", "var(--color-link)"],
  // 弹窗组件里的蓝色强调（与品牌主色同族）
  ["#3a6cf5", "var(--color-primary)"],
  ["#5a8bf7", "var(--color-primary)"],
];

/** rgba(r,g,b,a) 淡化色 → color-mix(in srgb, var(--token) P%, transparent) */
const RGBA_MAP = [
  [/rgba\(\s*57\s*,\s*108\s*,\s*216\s*,\s*([0-9.]+)\s*\)/g, "--color-primary"],
  [/rgba\(\s*229\s*,\s*72\s*,\s*77\s*,\s*([0-9.]+)\s*\)/g, "--color-danger"],
  [/rgba\(\s*209\s*,\s*52\s*,\s*56\s*,\s*([0-9.]+)\s*\)/g, "--color-danger"],
  [/rgba\(\s*34\s*,\s*197\s*,\s*94\s*,\s*([0-9.]+)\s*\)/g, "--color-ok-vivid"],
  [/rgba\(\s*224\s*,\s*49\s*,\s*49\s*,\s*([0-9.]+)\s*\)/g, "--color-danger-text"],
  [/rgba\(\s*47\s*,\s*158\s*,\s*68\s*,\s*([0-9.]+)\s*\)/g, "--color-ok-text"],
  [/rgba\(\s*180\s*,\s*83\s*,\s*9\s*,\s*([0-9.]+)\s*\)/g, "--color-warn-solid"],
];

function pct(a) {
  const v = Math.round(parseFloat(a) * 1000) / 10;
  return `${v}%`;
}

function transform(code) {
  let out = code;
  let n = 0;
  for (const [from, to] of HEX_MAP) {
    const parts = out.split(from);
    if (parts.length > 1) {
      n += parts.length - 1;
      out = parts.join(to);
    }
  }
  for (const [re, token] of RGBA_MAP) {
    out = out.replace(re, (_m, a) => {
      n += 1;
      return `color-mix(in srgb, var(${token}) ${pct(a)}, transparent)`;
    });
  }
  return { out, n };
}

function styleBlocks(code) {
  // 返回 [{start, end}] 的 <style> 内容区间（含标签内的内容，不含标签本身）
  const blocks = [];
  const re = /<style[^>]*>([\s\S]*?)<\/style>/g;
  let m;
  while ((m = re.exec(code))) {
    const start = m.index + m[0].indexOf(">") + 1;
    blocks.push([start, start + m[1].length]);
  }
  return blocks;
}

function walk(dir, acc = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p, acc);
    else if (/\.(vue|css)$/.test(e.name)) acc.push(p);
  }
  return acc;
}

const apply = process.argv.includes("--apply");
const files = walk(ROOT);
const report = [];
let total = 0;

for (const file of files) {
  if (SKIP_FILES.has(file)) continue;
  const code = fs.readFileSync(file, "utf8");
  const rel = path.relative(path.join(__dirname, ".."), file);

  if (file.endsWith(".css")) {
    const { out, n } = transform(code);
    if (n) {
      report.push(`${rel}: ${n}`);
      total += n;
      if (apply) fs.writeFileSync(file, out, "utf8");
    }
    continue;
  }

  // .vue：逐段只改 <style> 内容
  const blocks = styleBlocks(code);
  if (!blocks.length) continue;
  let updated = code;
  let count = 0;
  // 从后往前替换，避免偏移失效
  for (const [s, e] of blocks.reverse()) {
    const seg = updated.slice(s, e);
    const { out, n } = transform(seg);
    if (n) {
      count += n;
      updated = updated.slice(0, s) + out + updated.slice(e);
    }
  }
  if (count) {
    report.push(`${rel}: ${count}`);
    total += count;
    if (apply) fs.writeFileSync(file, updated, "utf8");
  }
}

report.sort();
console.log(report.join("\n"));
console.log(`\n${apply ? "已写入" : "dry-run"}：${report.length} 个文件，${total} 处替换`);
