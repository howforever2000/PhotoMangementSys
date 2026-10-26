const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };
const PERSONS = Array.from({ length: 12 }, (_, i) => ({ id: "p" + i, name: "人物" + i, face_count: 10 * i, created_at: "2024-01-02" }));

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  await p.addInitScript(({ user, persons }) => {
    const stub = async (cmd) => {
      if (cmd === "get_current_user") return user;
      if (/^(get_albums|list_albums|get_albums_with_stats|list_timeline|list_categories)$/.test(cmd)) return [];
      if (cmd === "list_persons") return persons;
      if (cmd === "get_person_avatars_bulk") return persons.map(() => null);
      if (cmd === "get_person_avatar") return null;
      if (cmd === "get_person_photos") return [];
      if (cmd === "category_index_stats") return { indexed: 0, stale: 0, known: 0, model: "", semantic_categories: 0 };
      return null;
    };
    window.__TAURI_INTERNALS__ = { invoke: stub, transformCallback: (cb) => cb, convertFileSrc: (x) => x,
      metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } } };
    window.__TAURI__ = { core: { invoke: stub, convertFileSrc: (x) => x },
      event: { listen: async () => () => {}, emit: async () => {} }, dialog: { open: async () => null } };
  }, { user: USER, persons: PERSONS });

  const measure = () => p.evaluate(() => new Promise((resolve) => {
    const frames = []; const long = [];
    const obs = new PerformanceObserver((l) => l.getEntries().forEach((e) => long.push(Math.round(e.duration))));
    try { obs.observe({ type: "longtask", buffered: false }); } catch (e) {}
    let last = performance.now(); const t0 = last;
    const tick = () => {
      const now = performance.now(); frames.push(now - last); last = now;
      if (now - t0 < 2500) requestAnimationFrame(tick);
      else { obs.disconnect();
        const s = frames.slice(2).sort((a, b) => a - b);
        resolve({ n: s.length, avg: +(s.reduce((a, c) => a + c, 0) / s.length).toFixed(1),
          p95: +s[Math.floor(s.length * 0.95)].toFixed(1), worst: +s[s.length - 1].toFixed(1),
          fps: +(1000 / (s.reduce((a, c) => a + c, 0) / s.length)).toFixed(1),
          longtasks: long });
      }
    };
    requestAnimationFrame(tick);
  }));

  // 点击 → 视觉响应延迟
  const clickLatency = async (sel) => {
    const t = await p.evaluate((s) => new Promise((resolve) => {
      const btn = document.querySelector(s);
      if (!btn) return resolve(null);
      const t0 = performance.now();
      let done = false;
      const obs = new MutationObserver(() => {
        if (done) return;
        done = true; obs.disconnect();
        requestAnimationFrame(() => resolve(+(performance.now() - t0).toFixed(1)));
      });
      obs.observe(document.body, { childList: true, subtree: true, attributes: true });
      btn.click();
      setTimeout(() => { if (!done) { done = true; obs.disconnect(); resolve(-1); } }, 3000);
    }), sel);
    return t;
  };

  const routes = [["/smart?tab=overview", "概览"], ["/smart?tab=face", "人物"], ["/albums", "相册"], ["/timeline", "时间线"]];
  for (const mat of ["plain", "liquid"]) {
    console.log(`\n########## mat=${mat} ##########`);
    for (const [route, name] of routes) {
      await p.goto(`http://localhost:${PORT}${route}`, { waitUntil: "load" });
      await p.waitForTimeout(500);
      await p.evaluate((m) => { const r = JSON.parse(localStorage.getItem("pm-theme") || "{}"); r.material = m; r.widgetMaterial = m === "liquid" ? "glass" : "plain"; localStorage.setItem("pm-theme", JSON.stringify(r)); }, mat);
      await p.reload({ waitUntil: "load" });
      await p.waitForTimeout(1800);
      const m = await measure();
      // 滚动时再测一遍
      await p.mouse.move(700, 450);
      for (let i = 0; i < 12; i++) { await p.mouse.wheel(0, 120); await p.waitForTimeout(40); }
      const ms = await measure();
      console.log(`  ${name} 静止: fps=${m.fps} avg=${m.avg}ms p95=${m.p95} worst=${m.worst} longtasks=${JSON.stringify(m.longtasks)}`);
      console.log(`  ${name} 滚动: fps=${ms.fps} avg=${ms.avg}ms p95=${ms.p95} worst=${ms.worst} longtasks=${JSON.stringify(ms.longtasks)}`);
    }
  }

  console.log("\n=== 点击→DOM 变化延迟（liquid） ===");
  await p.goto(`http://localhost:${PORT}/smart?tab=overview`, { waitUntil: "load" });
  await p.waitForTimeout(500);
  await p.evaluate(() => { const r = JSON.parse(localStorage.getItem("pm-theme") || "{}"); r.material = "liquid"; r.widgetMaterial = "glass"; localStorage.setItem("pm-theme", JSON.stringify(r)); });
  await p.reload({ waitUntil: "load" });
  await p.waitForTimeout(2000);
  console.log("  切 tab→人物:", await clickLatency('.tab, [role="tab"]'), "ms");

  await b.close();
})();
