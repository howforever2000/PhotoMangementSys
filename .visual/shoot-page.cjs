// 通用截图：node .visual/shoot-page.cjs <页面> [模式...]
// 例：node .visual/shoot-page.cjs AlbumList dark light
const { chromium } = require("playwright");

const page_ = process.argv[2] || "AlbumList";
const modes = process.argv.slice(3).length ? process.argv.slice(3) : ["dark", "light"];

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  for (const mode of modes) {
    const p = await b.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 1 });
    p.on("pageerror", (e) => console.log(`[${mode}] 错误`, String(e).split("\n")[0]));
    await p.goto(`http://localhost:5173/.visual/themed-probe.html#${mode}/${page_}`, { waitUntil: "load" });
    await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 25000 }).catch(() => {});
    await p.evaluate(() => document.fonts.ready);
    await p.waitForTimeout(900);

    const info = await p.evaluate((mode) => {
      const el = document.getElementById(`probe-${mode}`);
      if (!el) return null;
      const rc = el.getBoundingClientRect();
      return { top: rc.top + window.scrollY, height: rc.height };
    }, mode);
    if (info && info.height > 0) {
      const out = `design/preview/shot-${page_}-${mode}.png`;
      await p.screenshot({
        path: out,
        clip: { x: 0, y: info.top, width: 1440, height: Math.min(info.height, 900) },
        fullPage: true,
      });
      console.log("shot " + out);
    }
    await p.close();
  }
  await b.close();
})();
