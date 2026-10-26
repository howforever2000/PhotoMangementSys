const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };
const PERSONS = Array.from({ length: 6 }, (_, i) => ({ id: "p" + i, name: "人物" + i, face_count: 10 * i, created_at: "2024-01-02" }));

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

  const pct = (aB64, bB64) => p.evaluate(async ([x, y]) => {
    const load = (s) => new Promise((r) => { const i = new Image(); i.onload = () => r(i); i.src = "data:image/png;base64," + s; });
    const [ia, ib] = await Promise.all([load(x), load(y)]);
    const cv = document.createElement("canvas"); cv.width = ia.width; cv.height = ia.height;
    const g = cv.getContext("2d"); g.drawImage(ia, 0, 0);
    const da = g.getImageData(0, 0, cv.width, cv.height).data;
    g.clearRect(0, 0, cv.width, cv.height); g.drawImage(ib, 0, 0);
    const db = g.getImageData(0, 0, cv.width, cv.height).data;
    let n = 0, mx = 0;
    for (let i = 0; i < da.length; i += 4) {
      const d = Math.abs(da[i] - db[i]) + Math.abs(da[i+1] - db[i+1]) + Math.abs(da[i+2] - db[i+2]);
      if (d > 8) n++; if (d > mx) mx = d;
    }
    return { pct: +(((n / (da.length / 4)) * 100).toFixed(2)), maxDelta: mx };
  }, [aB64, bB64]);

  for (const mat of ["liquid", "plain"]) {
    await p.goto(`http://localhost:${PORT}/smart?tab=face`, { waitUntil: "load" });
    await p.waitForTimeout(500);
    await p.evaluate((m) => { const r = JSON.parse(localStorage.getItem("pm-theme") || "{}"); r.material = m; localStorage.setItem("pm-theme", JSON.stringify(r)); }, mat);
    await p.reload({ waitUntil: "load" });
    await p.waitForTimeout(1800);

    const box = await p.locator(".person-card").first().boundingBox();
    const clip = { x: box.x - 6, y: box.y - 6, width: box.width + 12, height: box.height + 12 };
    const before = (await p.screenshot({ clip })).toString("base64");
    // 用与 main.css 现有渐变完全一致的方式补回棱环底座
    await p.addStyleTag({ content: `
      body.mat-liquid .glass-card::after {
        content: ""; position: absolute; inset: 0; border-radius: inherit;
        pointer-events: none; z-index: 2;
      }` });
    await p.waitForTimeout(500);
    const after = (await p.screenshot({ clip })).toString("base64");
    const afterPE = await p.evaluate(() => getComputedStyle(document.querySelector(".person-card"), "::after").pointerEvents);
    console.log(`[mat=${mat}] 补回棱环后像素差 = ${JSON.stringify(await pct(before, after))}  伪元素PE=${afterPE}`);

    // 棱环存在时点击是否仍正常
    const v0 = await p.locator(".viewer-mask").count();
    await p.locator(".person-avatar-wrap").first().click({ timeout: 2500 }).catch(() => {});
    await p.waitForTimeout(500);
    console.log(`  棱环存在时点头像 → viewer ${v0} → ${await p.locator(".viewer-mask").count()}`);
  }
  await b.close();
})();
