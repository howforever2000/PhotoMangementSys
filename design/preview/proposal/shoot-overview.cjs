// 智慧相册·全局概览效果图：node design/preview/proposal/shoot-overview.cjs
// 变体：暮紫·玻璃 / 松绿·玻璃（混色渐变）/ 暮紫·普通材质
const { chromium } = require("playwright");
const path = require("path");

const VARIANTS = [
  ["glass", "mock-overview-dusk-glass.png"],
  ["pine", "mock-overview-pine-glass.png"],
  ["plain", "mock-overview-dusk-plain.png"],
];

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  for (const [v, out] of VARIANTS) {
    const p = await b.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 2 });
    p.on("pageerror", (e) => console.log(`[${v}] 错误`, String(e).split("\n")[0]));
    const file = "file:///" + path.resolve(__dirname, "mock-smart-overview.html").replace(/\\/g, "/");
    await p.goto(`${file}?v=${v}`, { waitUntil: "load" });
    await p.evaluate(() => document.fonts.ready);
    await p.waitForTimeout(300);
    await p.screenshot({ path: path.join(__dirname, out), fullPage: true });
    console.log("shot", out);
    await p.close();
  }
  await b.close();
})();
