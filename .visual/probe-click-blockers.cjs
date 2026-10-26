const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };
const PERSONS = [
  { id: "p01", name: "张三", face_count: 128, created_at: "2024-01-02" },
  { id: "p02", name: "李四", face_count: 96, created_at: "2024-02-11" },
  { id: "p03", name: "王五", face_count: 74, created_at: "2024-03-05" },
];
const ALBUMS = [
  { id: 1, name: "旅行", cover: null, photo_count: 12, created_at: 1700000000 },
  { id: 2, name: "家人", cover: null, photo_count: 8, created_at: 1700000100 },
];

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + String(e).split("\n")[0]));
  p.on("console", (m) => { if (m.type() === "error" && !/local resource/.test(m.text())) errs.push("console: " + m.text().slice(0, 160)); });

  await p.addInitScript(({ user, persons, albums }) => {
    const stub = async (cmd) => {
      if (cmd === "get_current_user") return user;
      if (cmd === "get_albums" || cmd === "list_albums" || cmd === "get_albums_with_stats") return albums;
      if (cmd === "list_timeline") return [];
      if (cmd === "list_categories") return [];
      if (cmd === "list_persons") return persons;
      if (cmd === "get_person_avatars_bulk") return persons.map(() => null);
      if (cmd === "get_person_avatar") return null;
      if (cmd === "get_person_photos") return [];
      if (cmd === "list_album_photos") return [];
      if (cmd === "category_index_stats") return { indexed: 0, stale: 0, known: 0, model: "", semantic_categories: 0 };
      return null;
    };
    window.__TAURI_INTERNALS__ = { invoke: stub, transformCallback: (cb) => cb, convertFileSrc: (x) => x,
      metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } } };
    window.__TAURI__ = { core: { invoke: stub, convertFileSrc: (x) => x },
      event: { listen: async () => () => {}, emit: async () => {} }, dialog: { open: async () => null } };
  }, { user: USER, persons: PERSONS, albums: ALBUMS });

  /**
   * 扫描页面上所有可点元素：在元素中心做 elementsFromPoint，
   * 检查最顶层命中元素是否落在该可点元素内部（否则视为「被遮挡/点击被吞」）。
   */
  async function scan(label) {
    const res = await p.evaluate(() => {
      const clickables = [...document.querySelectorAll(
        'button, a, [role="button"], input, select, textarea, .person-avatar-wrap, .hit-area'
      )];
      const bad = [];
      for (const el of clickables) {
        const r = el.getBoundingClientRect();
        if (r.width < 2 || r.height < 2) continue;
        if (r.bottom < 0 || r.top > innerHeight || r.right < 0 || r.left > innerWidth) continue;
        const cs = getComputedStyle(el);
        if (cs.visibility === "hidden" || cs.display === "none" || cs.opacity === "0") continue;
        if (el.disabled) continue;
        const cx = r.left + r.width / 2, cy = r.top + r.height / 2;
        const top = document.elementFromPoint(cx, cy);
        if (!top) { bad.push({ why: "null", tag: el.tagName, cls: String(el.className).slice(0, 50), txt: (el.innerText || "").slice(0, 16) }); continue; }
        if (top === el || el.contains(top)) continue;
        // 最顶层不是自己 → 记录遮挡者
        const chain = []; let n = top;
        while (n && chain.length < 4) { chain.push(n.tagName.toLowerCase() + (n.className ? "." + String(n.className).split(" ").slice(0, 2).join(".") : "")); n = n.parentElement; }
        bad.push({
          tag: el.tagName, cls: String(el.className).slice(0, 50), txt: (el.innerText || "").slice(0, 16),
          blocker: chain.join(" < "),
        });
      }
      return { total: clickables.length, bad };
    });
    console.log(`  [${label}] 可点元素 ${res.total} 个，被遮挡 ${res.bad.length} 个`);
    res.bad.slice(0, 12).forEach((x) => console.log(`     ✗ <${x.tag}> .${x.cls} "${x.txt}" ← 遮挡者 ${x.blocker}`));
    return res;
  }

  const setTheme = async (mat, wm) => {
    await p.evaluate(([m, w]) => {
      const r = JSON.parse(localStorage.getItem("pm-theme") || "{}");
      r.material = m; r.widgetMaterial = w;
      localStorage.setItem("pm-theme", JSON.stringify(r));
    }, [mat, wm]);
  };

  const pages = [
    ["/home", "主页"],
    ["/smart?tab=face", "人物"],
    ["/smart?tab=overview", "概览"],
    ["/albums", "相册列表"],
    ["/timeline", "时间线"],
    ["/memories", "回忆"],
  ];

  for (const mat of ["plain", "liquid"]) {
    console.log(`\n############ 材质 = ${mat} ############`);
    for (const [route, name] of pages) {
      await p.goto(`http://localhost:${PORT}${route}`, { waitUntil: "load" });
      await p.waitForTimeout(600);
      await setTheme(mat, mat === "liquid" ? "glass" : "plain");
      await p.reload({ waitUntil: "load" });
      await p.waitForTimeout(1800);
      await scan(`${name} ${route}`);
    }
    // 主题弹窗内部
    await p.goto(`http://localhost:${PORT}/home`, { waitUntil: "load" });
    await p.waitForTimeout(1600);
    await p.locator(".icon-btn").first().click();
    await p.waitForTimeout(500);
    await scan("主题弹窗·预定义页");
    await p.locator(".pm-tab", { hasText: "自定义" }).first().click();
    await p.waitForTimeout(400);
    await scan("主题弹窗·自定义页");
    await p.keyboard.press("Escape");
    await p.waitForTimeout(300);
  }

  console.log("\nerrors:", errs.slice(0, 8).join(" | ") || "(none)");
  await b.close();
})();
