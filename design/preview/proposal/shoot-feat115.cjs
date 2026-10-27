// FEAT-115 效果图截图：node design/preview/proposal/shoot-feat115.cjs
// 输出 design/preview/proposal/feat115-*.png（1440×900 @2x）
// 深浅两套：pine-soft（松夜绿·低保和）/ celadon-soft（晨雾青瓷）
const { chromium } = require("playwright");
const path = require("path");

const CHROME = "/home/hao/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome";

const PAGES = [
  ["feat115-albums", "相册管理"],
  ["feat115-smart-overview", "智慧相册 · 全局概览"],
  ["feat115-smart-cards", "内容分类 · 人物"],
  ["feat115-timeline", "时间线（基准页）"],
  ["feat115-scan", "扫描入库"],
  ["feat115-memories-search-detail", "回忆 · 智能搜索 · 相册详情"],
];
const THEMES = [
  ["pine-soft", "松夜绿"],
  ["celadon-soft", "晨雾青瓷"],
];

(async () => {
  const b = await chromium.launch({ executablePath: CHROME, args: ["--no-sandbox"] });
  for (const [theme, themeName] of THEMES) {
    for (const [name, label] of PAGES) {
      const p = await b.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });
      p.on("pageerror", (e) => console.log(`[${name}/${theme}] 错误`, String(e).split("\n")[0]));
      p.on("console", (m) => {
        if (m.type() === "error") console.log(`[${name}/${theme}] console.error`, m.text().slice(0, 200));
      });
      const file = "file:///" + path.resolve(__dirname, `${name}.html`).replace(/\\/g, "/");
      await p.goto(file, { waitUntil: "load" });
      await p.evaluate((t) => { document.body.dataset.theme = t; }, theme);
      await p.evaluate(() => document.fonts.ready);
      await p.waitForTimeout(420);
      const out = path.join(__dirname, `${name}-${theme}.png`);
      await p.screenshot({ path: out, fullPage: true });
      console.log("shot", path.basename(out), `(${themeName})`);
      await p.close();
    }
  }
  await b.close();
})();
