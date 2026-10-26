const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };
const PERSONS = [
  { id: "p01", name: "张三", face_count: 128, created_at: "2024-01-02" },
  { id: "p02", name: "李四", face_count: 96, created_at: "2024-02-11" },
];
const ALBUMS = [
  { id: 1, name: "旅行", cover: null, photo_count: 12, created_at: 1700000000 },
  { id: 2, name: "家人", cover: null, photo_count: 8, created_at: 1700000100 },
];

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  await p.addInitScript(({ user, persons, albums }) => {
    const stub = async (cmd) => {
      if (cmd === "get_current_user") return user;
      if (/^(get_albums|list_albums|get_albums_with_stats)$/.test(cmd)) return albums;
      if (/^(list_timeline|list_categories)$/.test(cmd)) return [];
      if (cmd === "list_persons") return persons;
      if (cmd === "get_person_avatars_bulk") return persons.map(() => null);
      if (cmd === "get_person_avatar") return null;
      if (cmd === "get_person_photos" || cmd === "list_album_photos") return [];
      if (cmd === "category_index_stats") return { indexed: 0, stale: 0, known: 0, model: "", semantic_categories: 0 };
      return null;
    };
    window.__TAURI_INTERNALS__ = { invoke: stub, transformCallback: (cb) => cb, convertFileSrc: (x) => x,
      metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } } };
    window.__TAURI__ = { core: { invoke: stub, convertFileSrc: (x) => x },
      event: { listen: async () => () => {}, emit: async () => {} }, dialog: { open: async () => null } };
  }, { user: USER, persons: PERSONS, albums: ALBUMS });

  const diff = (a, c) => p.evaluate(async ([x, y]) => {
    const load = (s) => new Promise((r) => { const i = new Image(); i.onload = () => r(i); i.src = "data:image/png;base64," + s; });
    const [ia, ib] = await Promise.all([load(x), load(y)]);
    const cv = document.createElement("canvas"); cv.width = ia.width; cv.height = ia.height;
    const g = cv.getContext("2d"); g.drawImage(ia, 0, 0);
    const da = g.getImageData(0, 0, cv.width, cv.height).data;
    g.clearRect(0, 0, cv.width, cv.height); g.drawImage(ib, 0, 0);
    const db = g.getImageData(0, 0, cv.width, cv.height).data;
    let n = 0; for (let i = 0; i < da.length; i += 4) if (Math.abs(da[i]-db[i])+Math.abs(da[i+1]-db[i+1])+Math.abs(da[i+2]-db[i+2]) > 8) n++;
    return +(((n / (da.length / 4)) * 100).toFixed(2));
  }, [a, c]);

  const pages = [
    ["/home", "主页"],
    ["/smart?tab=face", "人物"],
    ["/smart?tab=overview", "概览"],
    ["/albums", "相册列表"],
    ["/timeline", "时间线"],
    ["/memories", "回忆"],
  ];

  for (const alpha of [0.3, 0.64, 0.85, 0.95]) {
    const row = [];
    for (const [route, name] of pages) {
      await p.goto(`http://localhost:${PORT}${route}`, { waitUntil: "load" });
      await p.waitForTimeout(500);
      const base = await p.evaluate((a) => {
        const r = JSON.parse(localStorage.getItem("pm-theme") || "{}");
        r.material = "plain"; r.widgetMaterial = "glass"; r.compAlpha = a;
        localStorage.setItem("pm-theme", JSON.stringify(r));
      }, alpha);
      await p.reload({ waitUntil: "load" });
      await p.waitForTimeout(1600);
      const s1 = (await p.screenshot({ type: "png" })).toString("base64");
      await p.evaluate(() => {
        const r = JSON.parse(localStorage.getItem("pm-theme") || "{}");
        r.material = "liquid";
        localStorage.setItem("pm-theme", JSON.stringify(r));
      });
      await p.reload({ waitUntil: "load" });
      await p.waitForTimeout(1600);
      const s2 = (await p.screenshot({ type: "png" })).toString("base64");
      row.push(`${name}=${await diff(s1, s2)}%`);
    }
    console.log(`compAlpha=${alpha}: ${row.join("  ")}`);
  }

  // 弹窗自身（用户点材质按钮时眼前那块）
  for (const alpha of [0.64, 0.85, 0.95]) {
    await p.goto(`http://localhost:${PORT}/home`, { waitUntil: "load" });
    await p.waitForTimeout(1200);
    await p.evaluate((a) => { const r = JSON.parse(localStorage.getItem("pm-theme") || "{}"); r.compAlpha = a; r.material = "plain"; localStorage.setItem("pm-theme", JSON.stringify(r)); }, alpha);
    await p.reload({ waitUntil: "load" });
    await p.waitForTimeout(1600);
    await p.locator(".icon-btn").first().click();
    await p.waitForTimeout(400);
    await p.locator(".pm-tab", { hasText: "自定义" }).first().click();
    await p.waitForTimeout(300);
    const rect = await p.locator(".pm-dialog").boundingBox();
    const s1 = (await p.screenshot({ clip: rect })).toString("base64");
    await p.locator(".pm-seg button", { hasText: "液态玻璃" }).first().click();
    await p.waitForTimeout(700);
    const s2 = (await p.screenshot({ clip: rect })).toString("base64");
    console.log(`弹窗自身 compAlpha=${alpha}: 像素差 ${await diff(s1, s2)}%`);
    await p.keyboard.press("Escape");
    await p.waitForTimeout(300);
  }

  await b.close();
})();
