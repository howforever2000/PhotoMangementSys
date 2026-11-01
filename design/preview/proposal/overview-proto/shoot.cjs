// 出图脚本：把「全局概览」现状与两套方案渲染成 PNG。
//
// 前置：仓库根目录起一个 vite dev（页面里用的是 TS + .vue 单文件组件）
//   npx vite --port 5199
// 出图：
//   node design/preview/proposal/overview-proto/shoot.cjs
//
// 三个页面：
//   current.html —— 真实 SmartAlbum（后端 invoke 打桩，假快照）
//   index.html?v=a —— 方案 A 原型（真实 WidgetGrid / UiWidget / *Core）
//   index.html?v=b —— 方案 B 原型
const { chromium } = require("playwright");
const path = require("path");

const BASE = process.env.PREVIEW_BASE || "http://localhost:5199";
const OUT = path.resolve(__dirname, "..");
const W = 1600;
const H = 1000;

const PAGES = [
  { url: "/design/preview/proposal/overview-proto/current.html", out: "overview-current.png" },
  { url: "/design/preview/proposal/overview-proto/index.html?v=a", out: "overview-a.png" },
  { url: "/design/preview/proposal/overview-proto/index.html?v=b", out: "overview-b.png" },
];

(async () => {
  const browser = await chromium.launch();
  const errors = [];
  for (const p of PAGES) {
    const page = await browser.newPage({ viewport: { width: W, height: H }, deviceScaleFactor: 2 });
    page.on("pageerror", (e) => errors.push(`${p.out}: ${e.message}`));
    page.on("console", (m) => {
      if (m.type() === "error") errors.push(`${p.out} console: ${m.text()}`);
    });
    await page.goto(BASE + p.url, { waitUntil: "load", timeout: 30000 });
    await page.waitForTimeout(1200);
    await page.screenshot({ path: path.join(OUT, p.out), fullPage: true });
    console.log("→", p.out);
    await page.close();
  }
  if (errors.length) console.log("errors:", errors.slice(0, 6));
  await browser.close();
})();
