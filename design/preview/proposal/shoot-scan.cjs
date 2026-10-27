// 扫描入库效果图：node design/preview/proposal/shoot-scan.cjs
const { chromium } = require("playwright");
const path = require("path");

const FILE = "file:///" + path.resolve(__dirname, "mock-scan-rework.html").replace(/\\/g, "/");
const SHOTS = [
  ["", "mock-scan-dusk.png"],
  ["?v=pine", "mock-scan-pine.png"],
  ["?state=running", "mock-scan-running.png"],
];

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  for (const [q, out] of SHOTS) {
    const p = await b.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 2 });
    p.on("pageerror", (e) => console.log(`[${q}] 错误`, String(e).split("\n")[0]));
    await p.goto(`${FILE}${q}`, { waitUntil: "load" });
    await p.evaluate(() => document.fonts.ready);
    await p.waitForTimeout(300);
    await p.screenshot({ path: path.join(__dirname, out), fullPage: true });
    console.log("shot", out);
    await p.close();
  }
  await b.close();
})();
