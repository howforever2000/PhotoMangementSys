// 液态玻璃对照页截图 + 读性能读数
//   $env:VITE_PORT=1519; node .visual/shoot-glass-lab.cjs
const { chromium } = require("playwright");

const PORT = process.env.VITE_PORT || "1420";

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 2 });
  p.on("pageerror", (e) => console.log("pageerror", String(e).split("\n")[0]));
  p.on("console", (m) => {
    if (m.type() === "error") console.log("console.error", m.text().slice(0, 200));
  });

  await p.goto(`http://localhost:${PORT}/.visual/glass-refraction-lab.html`, { waitUntil: "load" });
  await p.waitForFunction(() => window.__GLASS_PROBE_READY__ === true, { timeout: 25000 }).catch(() =>
    console.log("(性能探针未在期限内完成，截图仍会输出)"),
  );
  await p.evaluate(() => document.fonts.ready);
  await p.waitForTimeout(500);

  // 三档的 computed backdrop-filter 是否真的生效（无效值会被丢弃成 none）
  const state = await p.evaluate(() => {
    const pick = (id) => {
      const el = document.getElementById(id);
      if (!el) return { id, missing: true };
      const cs = getComputedStyle(el);
      return { id, backdropFilter: cs.backdropFilter };
    };
    return {
      g1: pick("g1"),
      g2: pick("g2"),
      g3: pick("g3"),
      tb: pick("tb-refract"),
      perf: {
        p1: document.getElementById("p1")?.textContent,
        p2: document.getElementById("p2")?.textContent,
        p3: document.getElementById("p3")?.textContent,
      },
    };
  });
  console.log(JSON.stringify(state, null, 1));

  await p.screenshot({ path: "design/preview/_glass-lab.png", fullPage: true });
  console.log("shot design/preview/_glass-lab.png");
  await b.close();
})();
