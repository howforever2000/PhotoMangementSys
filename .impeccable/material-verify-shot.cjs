/**
 * 三材质 + 两组预设 + 色温三档的渲染复核（纯取图，不改任何代码）
 *
 * 用法：先起 dev server（端口 1420），再 node .impeccable/material-verify-shot.cjs
 * 产出：.impeccable/review/mat-*.png / theme-*.png
 *
 * 复核的是「肉眼该能分辨」的三件事：
 *   1. 三材质在同一页面上是否仍可一眼区分（M1 雾 / M2 折射 / M3 开片）
 *   2. 素色系 5 套是否真的比经典组素
 *   3. 色温三档是否真的在动（且语义色没跟着动）
 */
const { chromium } = require("playwright");
const path = require("path");
const fs = require("fs");
const { INIT } = require("./mock-init.cjs");

const OUT = path.join(__dirname, "review");
fs.mkdirSync(OUT, { recursive: true });

const PRESETS = [
  ["classic-sage", { gradFrom: "#ffd7d7", gradMid: "#7e9963", gradTo: "#195e53", compColor: "#f2f9f4", compAlpha: 0.55 }],
  ["muted-linen", { gradFrom: "#f1eee9", gradMid: "#c6bba9", gradTo: "#5d503c", compColor: "#ededed", compAlpha: 0.62 }],
  ["muted-ink", { gradFrom: "#e9eef1", gradMid: "#92acbf", gradTo: "#2c3e4e", compColor: "#ececec", compAlpha: 0.62 }],
];

const MATERIALS = ["frosted", "liquid", "glazed"];
const TEMPS = [-1, 0, 1];

/** 写一份 localStorage 偏好后打开 /home */
async function shot(page, prefs, file) {
  await page.addInitScript((p) => {
    localStorage.setItem("pm-theme", JSON.stringify(p));
    localStorage.removeItem("pm-theme-image");
  }, prefs);
  await page.goto("http://localhost:1420/home", { waitUntil: "networkidle", timeout: 20000 });
  await page.waitForTimeout(900);
  // 读回实际生效的材质/色温，确认截图对应的是设定值而不是残留
  const state = await page.evaluate(() => ({
    mat: document.body.className,
    link: getComputedStyle(document.body).getPropertyValue("--color-link").trim(),
    primary: getComputedStyle(document.body).getPropertyValue("--color-primary").trim(),
    danger: getComputedStyle(document.body).getPropertyValue("--color-danger-text").trim(),
    ok: getComputedStyle(document.body).getPropertyValue("--color-ok-text").trim(),
  }));
  await page.screenshot({ path: path.join(OUT, file), fullPage: false });
  return state;
}

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 820 }, deviceScaleFactor: 2 });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(e.message));
  page.on("console", (m) => { if (m.type() === "error") errs.push(m.text()); });

  const BASE = {
    mode: "dark", bgStyle: "gradient", gradAngle: 180,
    saturation: 1, material: "frosted", bgOpacity: 0.55,
    bgColor: "#1a1428", gradFrom: "#3b2b52", gradMid: "#271d37", gradTo: "#120e1c",
    compColor: "#2a2630", compAlpha: 0.64, temperature: 0,
  };

  // ① 三材质 × 素色系「亚麻暖灰」
  for (const m of MATERIALS) {
    const s = await shot(page, { ...BASE, ...PRESETS[1][1], material: m }, `mat-${m}.png`);
    console.log(`mat-${m}: body="${s.mat}" link=${s.link} primary=${s.primary}`);
  }

  // ② 经典 vs 素色（同为磨砂，看色相是否真的收敛）
  await shot(page, { ...BASE, ...PRESETS[0][1], material: "frosted" }, "theme-classic.png");
  console.log("theme-classic: ok");
  await shot(page, { ...BASE, ...PRESETS[2][1], material: "frosted" }, "theme-muted-ink.png");
  console.log("theme-muted-ink: ok");

  // ③ 色温三档（语义色必须逐档不变）
  for (const t of TEMPS) {
    const s = await shot(page, { ...BASE, ...PRESETS[1][1], temperature: t }, `temp-${t}.png`);
    console.log(`temp-${t}: primary=${s.primary} danger=${s.danger} ok=${s.ok} link=${s.link}`);
  }

  await browser.close();
  console.log("CONSOLE_ERRORS:", errs.length ? [...new Set(errs)].slice(0, 8).join("\n") : "(none)");
})();
