// 里效果图：node design/preview/proposal/shoot.cjs
// 输出 design/preview/proposal/*.png（1440×900，与全站视觉基线同口径）
const { chromium } = require("playwright");
const path = require("path");

const PAGES = ["mock-albums", "mock-albums-light", "mock-location", "mock-timeline"];

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  for (const name of PAGES) {
    const p = await b.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });
    p.on("pageerror", (e) => console.log(`[${name}] 错误`, String(e).split("\n")[0]));
    const file = "file:///" + path.resolve(__dirname, `${name}.html`).replace(/\\/g, "/");
    await p.goto(file, { waitUntil: "load" });
    await p.evaluate(() => document.fonts.ready);
    await p.waitForTimeout(320);
    const out = path.join(__dirname, `${name}.png`);
    await p.screenshot({ path: out, fullPage: true });
    console.log("shot", path.basename(out));
    await p.close();
  }
  await b.close();
})();
