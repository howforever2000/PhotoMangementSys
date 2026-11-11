// 全局概览布局优化效果图：node design/preview/proposal/shoot-overview-layout.cjs
// 变体：现状对照 / 方案A 竖幅锚点 / 方案B 精排 / 方案C 记忆为冕（+ 松绿下的方案A）
const { chromium } = require("playwright");
const path = require("path");

const VARIANTS = [
  ["current", "", "overview-layout-current.png"],
  ["a", "", "overview-layout-a.png"],
  ["b", "", "overview-layout-b.png"],
  ["c", "", "overview-layout-c.png"],
  ["a", "pine", "overview-layout-a-pine.png"],
];

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  for (const [v, t, out] of VARIANTS) {
    const p = await b.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 2 });
    p.on("pageerror", (e) => console.log(`[${v}${t}] 错误`, String(e).split("\n")[0]));
    const file = "file:///" + path.resolve(__dirname, "mock-overview-layout.html").replace(/\\/g, "/");
    const q = new URLSearchParams({ v, ...(t ? { t } : {}) }).toString();
    await p.goto(`${file}?${q}`, { waitUntil: "load" });
    await p.evaluate(() => document.fonts.ready);
    await p.waitForTimeout(300);
    await p.screenshot({ path: path.join(__dirname, out), fullPage: true });
    console.log("shot", out);
    await p.close();
  }
  await b.close();
})();
