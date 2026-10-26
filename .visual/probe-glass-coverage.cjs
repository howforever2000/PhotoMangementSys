const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };
const ALBUMS = Array.from({ length: 6 }, (_, i) => ({ id: i + 1, name: "相册" + i, cover: null, photo_count: 5 * i, created_at: 1700000000 }));

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  await p.addInitScript(({ user, albums }) => {
    const stub = async (cmd) => {
      if (cmd === "get_current_user") return user;
      if (/^(get_albums|list_albums|get_albums_with_stats)$/.test(cmd)) return albums;
      if (/^(list_timeline|list_categories|list_persons)$/.test(cmd)) return [];
      if (cmd === "get_person_avatars_bulk") return [];
      if (cmd === "category_index_stats") return { indexed: 0, stale: 0, known: 0, model: "", semantic_categories: 0 };
      return null;
    };
    window.__TAURI_INTERNALS__ = { invoke: stub, transformCallback: (cb) => cb, convertFileSrc: (x) => x,
      metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } } };
    window.__TAURI__ = { core: { invoke: stub, convertFileSrc: (x) => x },
      event: { listen: async () => () => {}, emit: async () => {} }, dialog: { open: async () => null } };
  }, { user: USER, albums: ALBUMS });

  // 哪些元素真正"消费"了材质？统计全站 .glass-card / .glass-surface 数量 + 每页实际生效的 backdropFilter 元素数
  for (const [route, name] of [["/albums", "相册列表"], ["/timeline", "时间线"], ["/memories", "回忆"], ["/smart?tab=overview", "概览"]]) {
    await p.goto(`http://localhost:${PORT}${route}`, { waitUntil: "load" });
    await p.waitForTimeout(500);
    await p.evaluate(() => { const r = JSON.parse(localStorage.getItem("pm-theme") || "{}"); r.material = "liquid"; r.widgetMaterial = "glass"; localStorage.setItem("pm-theme", JSON.stringify(r)); });
    await p.reload({ waitUntil: "load" });
    await p.waitForTimeout(1800);
    const info = await p.evaluate(() => {
      const all = [...document.querySelectorAll("*")];
      let glassCls = 0, bfActive = 0, cardBg = 0;
      const clsSamples = [];
      for (const el of all) {
        if (el.classList.contains("glass-card") || el.classList.contains("glass-surface")) { glassCls++; if (clsSamples.length < 4) clsSamples.push(el.className.slice(0, 40)); }
        const cs = getComputedStyle(el);
        if (cs.backdropFilter && cs.backdropFilter !== "none") bfActive++;
      }
      return { glassCls, bfActive, clsSamples };
    });
    console.log(`  [${name}] glass 类元素=${info.glassCls} 生效 backdrop-filter 元素=${info.bfActive} 样例=${JSON.stringify(info.clsSamples)}`);
  }
  await b.close();
})();
