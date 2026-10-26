const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };
const PERSONS = Array.from({ length: 9 }, (_, i) => ({ id: "p" + i, name: "人物" + i, face_count: 10 * i, created_at: "2024-01-02" }));
const ALBUMS = Array.from({ length: 6 }, (_, i) => ({ id: i + 1, name: "相册" + i, cover: null, photo_count: 5 * i, created_at: 1700000000 }));

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

  // 逐点比对「最顶层命中元素」：plain vs liquid 有差异即说明材质改变了命中测试
  const hitMap = () => p.evaluate(() => {
    const pts = [];
    for (let y = 40; y < innerHeight; y += 40) for (let x = 20; x < innerWidth; x += 40) {
      const el = document.elementFromPoint(x, y);
      if (!el) { pts.push(x + "," + y + "=null"); continue; }
      const sig = el.tagName.toLowerCase() + "." + (String(el.className || "").split(" ").filter(Boolean).slice(0, 2).join("."));
      pts.push(x + "," + y + "=" + sig);
    }
    return pts;
  });

  const setT = (mat) => p.evaluate((m) => {
    const r = JSON.parse(localStorage.getItem("pm-theme") || "{}");
    r.material = m; r.widgetMaterial = m === "liquid" ? "glass" : "plain";
    localStorage.setItem("pm-theme", JSON.stringify(r));
  }, mat);

  const routes = [["/home", "主页"], ["/smart?tab=overview", "概览"], ["/smart?tab=face", "人物"], ["/albums", "相册列表"], ["/timeline", "时间线"], ["/memories", "回忆"], ["/location", "地点"]];

  for (const [route, name] of routes) {
    await p.goto(`http://localhost:${PORT}${route}`, { waitUntil: "load" });
    await p.waitForTimeout(500);
    await setT("plain");
    await p.reload({ waitUntil: "load" });
    await p.waitForTimeout(1700);
    const plain = await hitMap();
    await setT("liquid");
    await p.reload({ waitUntil: "load" });
    await p.waitForTimeout(1700);
    const liquid = await hitMap();
    const diffs = [];
    for (let i = 0; i < plain.length; i++) if (plain[i] !== liquid[i]) diffs.push(`${plain[i]}  →  ${liquid[i].split("=")[1]}`);
    console.log(`  [${name} ${route}] 采样 ${plain.length} 点，命中差异 ${diffs.length} 处`);
    diffs.slice(0, 8).forEach((d) => console.log(`     ✗ ${d}`));
  }

  // 主题弹窗开着时同样比对（材质按钮就在里面）
  for (const tab of ["预定义", "自定义"]) {
    await p.goto(`http://localhost:${PORT}/home`, { waitUntil: "load" });
    await p.waitForTimeout(1500);
    await p.locator(".icon-btn").first().click();
    await p.waitForTimeout(400);
    await p.locator(".pm-tab", { hasText: tab }).first().click();
    await p.waitForTimeout(400);
    const plain = await hitMap();
    const seg = await p.locator(".pm-seg button", { hasText: "液态玻璃" }).count();
    if (seg) {
      await p.locator(".pm-seg button", { hasText: "液态玻璃" }).first().click();
      await p.waitForTimeout(800);
      const liquid = await hitMap();
      const diffs = [];
      for (let i = 0; i < plain.length; i++) if (plain[i] !== liquid[i]) diffs.push(`${plain[i]}  →  ${liquid[i].split("=")[1]}`);
      console.log(`  [弹窗·${tab}] 命中差异 ${diffs.length} 处`);
      diffs.slice(0, 8).forEach((d) => console.log(`     ✗ ${d}`));
    } else {
      console.log(`  [弹窗·${tab}] 无液态按钮（此页无材质开关）`);
    }
    await p.keyboard.press("Escape");
  }

  await b.close();
})();
