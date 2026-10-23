#!/usr/bin/env node
/**
 * 视觉基线截图（FEAT-103 · P2~P5 验收依据）
 *
 * 为什么必须有脚本而不是手动截图：
 *   本次重设计每阶段都在动"字阶/间距/圆角/材质厚度"，这些改动的视觉差异
 *   只有几 px —— 肉眼比对同一屏的两次手拍基本不可靠（尤其浅色/深色两份）。
 *   有固定视口 + 固定数据 + dpr=2 的脚本后，任何一阶段的 diff 都能量化。
 *
 * 用法：
 *   node scripts/visual-baseline.mjs                # 拍到 .visual/shots/
 *   node scripts/visual-baseline.mjs --compare      # 与 .visual/baseline/ 比对
 *   node scripts/visual-baseline.mjs --update       # 当前结果存为基线
 *
 * 前置：需要先起 dev server（探针页）：`npx vite --port 5199`
 */

import { chromium } from "playwright";
import { mkdir, readdir, writeFile, cp, stat } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { PNG } from "pngjs";
import pixelmatch from "pixelmatch";

const BASE = process.env.BASE_URL ?? "http://localhost:5199";
const OUT = ".visual/shots";
const BASE_DIR = ".visual/baseline";

/** 待拍页面：探针页（组件级）与真实页面（P2 起加入） */
const TARGETS = [
  { name: "probe", path: "/.visual/ui-probe.html" },
  { name: "map", path: "/.visual/map-probe.html" },
];

/** 视口矩阵：宽屏 / 中窗 / 窄窗 × 深浅 */
const VIEWPORTS = [
  { w: 1440, h: 900, tag: "1440" },
  { w: 1280, h: 800, tag: "1280" },
  { w: 1024, h: 768, tag: "1024" },
];
const THEMES = ["dark", "light"];

const args = new Set(process.argv.slice(2));

async function ensureDir(d) {
  await mkdir(d, { recursive: true });
}

/** 通过 hash 让探针页切主题（真实页面则由 localStorage 里的 pm-theme 决定） */
function urlFor(t, theme) {
  const u = new URL(t.path, BASE);
  if (theme === "light") u.hash = "light";
  return u.toString();
}

async function shoot() {
  await ensureDir(OUT);
  const browser = await chromium.launch();
  const results = [];
  const errors = [];

  for (const t of TARGETS) {
    for (const vp of VIEWPORTS) {
      for (const theme of THEMES) {
        const page = await browser.newPage({
          viewport: { width: vp.w, height: vp.h },
          deviceScaleFactor: 2,
        });
        page.on("pageerror", (e) => errors.push(`${t.name}/${vp.tag}/${theme}: ${e.message}`));

        await page.goto(urlFor(t, theme), { waitUntil: "networkidle" });
        // 探针页自己按 hash 切主题；真实页面靠 body 类。两种都兜住。
        await page.evaluate((th) => {
          document.body.classList.toggle("theme-dark", th === "dark");
        }, theme);
        // 等字体落定，否则每次截图字宽会差 1~2px，diff 噪音极大
        await page.evaluate(() => document.fonts.ready);
        await page.waitForTimeout(250);

        const file = join(OUT, `${t.name}-${vp.tag}-${theme}.png`);
        await page.screenshot({ path: file, animations: "disabled" });
        results.push({ file, name: `${t.name}-${vp.tag}-${theme}` });
        await page.close();
      }
    }
  }

  await browser.close();
  return { results, errors };
}

async function compare(results) {
  await ensureDir(BASE_DIR);
  const report = [];
  let hasBaseline = false;

  for (const { file, name } of results) {
    const baseFile = join(BASE_DIR, `${name}.png`);
    if (!existsSync(baseFile)) continue;
    hasBaseline = true;

    const img1 = PNG.sync.read(await readFileBuf(baseFile));
    const img2 = PNG.sync.read(await readFileBuf(file));
    if (img1.width !== img2.width || img1.height !== img2.height) {
      report.push({ name, status: "尺寸变化", detail: `${img1.width}x${img1.height} → ${img2.width}x${img2.height}` });
      continue;
    }
    const { width, height } = img1;
    const diff = new PNG({ width, height });
    const changed = pixelmatch(img1.data, img2.data, diff.data, width, height, { threshold: 0.1 });
    const pct = ((changed / (width * height)) * 100).toFixed(3);
    if (changed > 0) {
      await writeFile(join(BASE_DIR, `${name}.diff.png`), PNG.sync.write(diff));
    }
    report.push({ name, status: changed === 0 ? "一致" : "有差异", detail: `${changed} px（${pct}%）` });
  }

  if (!hasBaseline) {
    console.log("⚠️  没有基线（.visual/baseline 为空）。先运行 --update 生成基线。");
    return;
  }
  console.table(report);
}

async function readFileBuf(p) {
  const { readFile } = await import("node:fs/promises");
  return readFile(p);
}

async function updateBaseline(results) {
  await ensureDir(BASE_DIR);
  for (const { file, name } of results) {
    await cp(file, join(BASE_DIR, `${name}.png`));
  }
  console.log(`已更新基线：${results.length} 张 → ${BASE_DIR}`);
}

(async () => {
  const { results, errors } = await shoot();
  console.log(`截图完成：${results.length} 张 → ${OUT}`);
  if (errors.length) {
    console.error("页面错误：\n" + errors.slice(0, 10).join("\n"));
    process.exitCode = 1;
  }
  if (args.has("--update")) {
    await updateBaseline(results);
  } else if (args.has("--compare")) {
    await compare(results);
  }
})();
