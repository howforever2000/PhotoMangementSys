// FEAT-110 全局概览截图：node .visual/shoot-overview.cjs
const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const VARIANTS = [
  ["dusk", "design/preview/proposal/shot-overview-real-dusk.png"],
  ["pine", "design/preview/proposal/shot-overview-real-pine.png"],
  ["plain", "design/preview/proposal/shot-overview-real-plain.png"],
];

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  for (const [v, out] of VARIANTS) {
    const p = await b.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 2 });
    p.on("pageerror", (e) => console.log(`[${v}] 错误`, String(e).split("\n")[0]));
    await p.goto(`http://localhost:${PORT}/.visual/overview-probe.html#${v}`, { waitUntil: "load" });
    await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 25000 }).catch(() => {});
    await p.evaluate(() => document.fonts.ready);
    await p.waitForTimeout(700);
    await p.screenshot({ path: out, fullPage: true });
    console.log("shot", out);
    await p.close();
  }
  await b.close();
})();
