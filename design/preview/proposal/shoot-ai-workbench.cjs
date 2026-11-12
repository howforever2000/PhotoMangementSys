// AI 修图工作台 效果图：node design/preview/proposal/shoot-ai-workbench.cjs
// 输出 design/preview/proposal/mock-ai-workbench*.png（1440 宽 @2x）
const { chromium } = require("playwright");
const path = require("path");

const PAGES = [
  { name: "mock-workshop-entry", h: 900 },
  { name: "mock-ai-workbench", h: 960 },
  { name: "mock-ai-workbench-album", h: 900 },
];

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  for (const { name, h } of PAGES) {
    const p = await b.newPage({ viewport: { width: 1440, height: h }, deviceScaleFactor: 2 });
    p.on("pageerror", (e) => console.log(`[${name}] 错误`, String(e).split("\n")[0]));
    const file = "file:///" + path.resolve(__dirname, `${name}.html`).replace(/\\/g, "/");
    await p.goto(file, { waitUntil: "load" });
    await p.evaluate(() => document.fonts.ready);
    await p.waitForTimeout(320);
    const out = path.join(__dirname, `${name}.png`);
    await p.screenshot({ path: out });
    console.log("shot", path.basename(out));
    await p.close();
  }
  await b.close();
})();
