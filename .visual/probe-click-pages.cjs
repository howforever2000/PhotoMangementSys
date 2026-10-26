const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };
const PERSONS = [
  { id: "p01", name: "张三", face_count: 128, created_at: "2024-01-02" },
  { id: "p02", name: "李四", face_count: 96, created_at: "2024-02-11" },
];
const PHOTOS = [
  { path: "C:/x/a.jpg", thumb: "C:/x/a_t.jpg", album_id: 1, taken_at: "2024-01-01", face_bbox: [10, 10, 100, 100] },
  { path: "C:/x/b.jpg", thumb: "C:/x/b_t.jpg", album_id: 1, taken_at: "2024-02-01", face_bbox: [10, 10, 100, 100] },
];

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + String(e).split("\n")[0]));
  p.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 160)); });

  await p.addInitScript(({ user, persons, photos }) => {
    const stub = async (cmd) => {
      if (cmd === "get_current_user") return user;
      if (cmd === "get_albums" || cmd === "list_albums" || cmd === "get_albums_with_stats") return [];
      if (cmd === "list_timeline" || cmd === "list_categories") return [];
      if (cmd === "list_persons") return persons;
      if (cmd === "get_person_avatars_bulk") return persons.map(() => null);
      if (cmd === "get_person_avatar") return null;
      if (cmd === "get_person_photos") return photos;
      if (cmd === "rename_person" || cmd === "merge_persons") return null;
      if (cmd === "category_index_stats") return { indexed: 0, stale: 0, known: 0, model: "", semantic_categories: 0 };
      return null;
    };
    window.__TAURI_INTERNALS__ = { invoke: stub, transformCallback: (cb) => cb, convertFileSrc: (x) => x,
      metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } } };
    window.__TAURI__ = { core: { invoke: stub, convertFileSrc: (x) => x },
      event: { listen: async () => () => {}, emit: async () => {} }, dialog: { open: async () => null } };
  }, { user: USER, persons: PERSONS, photos: PHOTOS });

  async function probe(label, locator, opts = {}) {
    const cnt = await locator.count();
    if (!cnt) return console.log(`  ${label}: 元素不存在 (count=0)`);
    const first = locator.first();
    if (opts.hoverFirst) { await first.hover({ timeout: 2000 }).catch(() => {}); await p.waitForTimeout(250); }
    const box = await first.boundingBox();
    if (!box) return console.log(`  ${label}: 无 boundingBox（不可见）`);
    const cx = box.x + box.width / 2, cy = box.y + box.height / 2;
    const hit = await p.evaluate(([x, y]) => {
      const el = document.elementFromPoint(x, y);
      const chain = []; let n = el;
      while (n && chain.length < 4) { chain.push(n.tagName.toLowerCase() + (n.className ? "." + String(n.className).split(" ").slice(0, 2).join(".") : "")); n = n.parentElement; }
      return { chain, pe: el ? getComputedStyle(el).pointerEvents : null };
    }, [cx, cy]);
    const before = p.url();
    let result = null;
    await first.click({ timeout: 2500 }).catch((e) => { result = "CLICK_FAIL:" + String(e).split("\n")[0].slice(0, 60); });
    await p.waitForTimeout(600);
    if (!result) {
      result = p.url() !== before ? "nav:" + p.url().replace(`http://localhost:${PORT}`, "") : "state:" + JSON.stringify(opts.state ? await opts.state() : null);
    }
    console.log(`  ${label}: size=${Math.round(box.width)}x${Math.round(box.height)} topEl=${hit.chain.slice(0, 3).join(" < ")} pe=${hit.pe} → ${result}`);
  }

  for (const mat of ["plain", "liquid"]) {
    console.log(`\n===== 人物页 mat=${mat} =====`);
    await p.goto(`http://localhost:${PORT}/smart?tab=face`, { waitUntil: "load" });
    await p.waitForTimeout(600);
    await p.evaluate((m) => { const r = JSON.parse(localStorage.getItem("pm-theme") || "{}"); r.material = m; localStorage.setItem("pm-theme", JSON.stringify(r)); }, mat);
    await p.reload({ waitUntil: "load" });
    await p.waitForTimeout(1800);
    console.log("  body:", await p.evaluate(() => document.body.className), " cards:", await p.locator(".person-card").count());

    await probe("① 头像点击", p.locator(".person-avatar-wrap"), { state: async () => ({ viewer: await p.locator(".viewer-mask").count() }) });
    if (await p.locator(".viewer-mask").count()) {
      await p.keyboard.press("Escape"); await p.waitForTimeout(350);
      if (await p.locator(".viewer-mask").count()) { await p.locator(".viewer-mask").click({ position: { x: 6, y: 6 } }); await p.waitForTimeout(350); }
      console.log("    → 关闭后 viewer:", await p.locator(".viewer-mask").count());
    }
    await probe("② 重命名 ✎", p.locator(".person-name-row .mini-btn"), { hoverFirst: true, state: async () => ({ renameInput: await p.locator(".person-rename-input").count() }) });
    if (await p.locator(".person-rename-input").count()) { await p.keyboard.press("Escape"); await p.waitForTimeout(300); }
    await probe("③ 合并", p.locator(".merge-btn"), { hoverFirst: true, state: async () => ({ mergeMask: await p.locator(".merge-mask").count() }) });
    if (await p.locator(".merge-mask").count()) { await p.locator(".merge-mask").click({ position: { x: 6, y: 6 } }); await p.waitForTimeout(300); }

    console.log(`\n===== 概览页 mat=${mat} wm=${mat === "liquid" ? "glass" : "plain"} =====`);
    await p.goto(`http://localhost:${PORT}/smart?tab=overview`, { waitUntil: "load" });
    await p.waitForTimeout(600);
    await p.evaluate((m) => { const r = JSON.parse(localStorage.getItem("pm-theme") || "{}"); r.material = m; r.widgetMaterial = m === "liquid" ? "glass" : "plain"; localStorage.setItem("pm-theme", JSON.stringify(r)); }, mat);
    await p.reload({ waitUntil: "load" });
    await p.waitForTimeout(2200);
    console.log("  body:", await p.evaluate(() => document.body.className), " widgets:", await p.locator(".ui-widget").count());

    await probe("④ 人物 widget", p.locator(".ui-widget--link").filter({ hasText: "人物照片覆盖" }));
    await p.goto(`http://localhost:${PORT}/smart?tab=overview`, { waitUntil: "load" });
    await p.waitForTimeout(2000);
    await probe("⑤ 相册 widget", p.locator(".ui-widget--link").filter({ hasText: "相册" }));
    await p.goto(`http://localhost:${PORT}/smart?tab=overview`, { waitUntil: "load" });
    await p.waitForTimeout(2000);
    await probe("⑥ 详情链接（语义索引）", p.locator(".ui-widget-link"));
    await p.goto(`http://localhost:${PORT}/smart?tab=overview`, { waitUntil: "load" });
    await p.waitForTimeout(2000);
    await probe("⑦ 那年今日（router-link to）", p.locator('a.ui-widget--link').first());
  }

  console.log("\nerrors:", errs.slice(0, 8).join(" | ") || "(none)");
  await b.close();
})();
