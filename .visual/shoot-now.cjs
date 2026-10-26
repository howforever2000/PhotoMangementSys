// 当前状态实拍（overview 探针）：node .visual/shoot-now.cjs dusk light pine
const { chromium } = require("playwright");
const fs = require("fs");
const PORT = process.env.VITE_PORT || "1519";
const OUT = ".visual/out";
fs.mkdirSync(OUT, { recursive: true });

const variants = process.argv.slice(2).length ? process.argv.slice(2) : ["dusk", "light"];

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  for (const v of variants) {
    const p = await b.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });
    p.on("pageerror", (e) => console.log(`[${v}] 错误`, String(e).split("\n")[0]));
    p.on("console", (m) => {
      if (m.type() === "error") console.log(`[${v}] console.error`, m.text().slice(0, 200));
    });
    await p.goto(`http://localhost:${PORT}/.visual/overview-probe.html#${v}`, { waitUntil: "load" });
    await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 30000 }).catch(() => console.log(`[${v}] 未就绪`));
    await p.evaluate(() => document.fonts.ready);
    await p.waitForTimeout(600);
    await p.screenshot({ path: `${OUT}/overview-${v}.png`, fullPage: true });
    await p.screenshot({ path: `${OUT}/overview-${v}-fold.png` });
    console.log("shot", `${OUT}/overview-${v}.png`);
    await p.close();
  }
  await b.close();
})();
