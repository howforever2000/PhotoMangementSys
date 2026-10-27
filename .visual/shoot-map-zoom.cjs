// 地点页缩放实拍：node .visual/shoot-map-zoom.cjs
const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 2 });
  p.on("pageerror", (e) => console.log("pageerror", String(e).split("\n")[0]));

  await p.goto(`http://localhost:${PORT}/.visual/themed-probe.html#dark/SmartAlbum`, { waitUntil: "load" });
  await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 25000 }).catch(() => {});
  await p.waitForTimeout(900);

  const tab = await p.$('.mh-nav-item:has-text("地点")');
  if (tab) { await tab.click(); await p.waitForTimeout(1800); }

  await p.evaluate(() => document.fonts.ready);
  await p.screenshot({ path: "design/preview/_map-zoom-1x.png" });

  const svg = await p.$(".u-map-svg");
  if (!svg) { console.log("no map"); await b.close(); return; }
  const box = await svg.boundingBox();
  // 瞄准西南角（达州/成都/贵阳聚成一团的地方）—— 比例按地图实际宽高算
  await p.mouse.move(
    Math.round(box.x + box.width * 0.47),
    Math.round(box.y + box.height * 0.72),
  );
  await p.keyboard.down("Control");
  for (let i = 0; i < 14; i++) await p.mouse.wheel(0, -260);
  await p.waitForTimeout(400);
  await p.keyboard.up("Control");
  await p.waitForTimeout(400);
  await p.screenshot({ path: "design/preview/_map-zoom-4x.png" });
  console.log("shot _map-zoom-1x.png / _map-zoom-4x.png");
  await b.close();
})();
