const { chromium } = require("playwright");
(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 1000 } });
  await p.goto("http://localhost:1420/.visual/themed-probe.html#dark/AlbumList", { waitUntil: "load" });
  await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 25000 }).catch(()=>{});
  await p.waitForTimeout(900);
  const r = await p.evaluate(() => {
    const rows = [...document.querySelectorAll(".album-wall-row")].map((row, ri) => ({
      ri, h: Math.round(row.getBoundingClientRect().height),
      cells: [...row.querySelectorAll(".album-wall-cell")].map(c => {
        const cov = c.querySelector(".tile-cover");
        return { w: Math.round(c.getBoundingClientRect().width), ch: Math.round(cov.getBoundingClientRect().height) };
      })
    }));
    return rows;
  });
  console.log(JSON.stringify(r, null, 1));
  await b.close();
})();
