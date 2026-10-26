const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };
const PERSONS = [
  { id: "p01", name: "张三", face_count: 128, created_at: "2024-01-02" },
  { id: "p02", name: "李四", face_count: 96, created_at: "2024-02-11" },
];

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

  for (const mat of ["plain", "liquid"]) {
    await p.goto(`http://localhost:${PORT}/smart?tab=face`, { waitUntil: "load" });
    await p.waitForTimeout(600);
    await p.evaluate((m) => { const r = JSON.parse(localStorage.getItem("pm-theme") || "{}"); r.material = m; localStorage.setItem("pm-theme", JSON.stringify(r)); }, mat);
    await p.reload({ waitUntil: "load" });
    await p.waitForTimeout(1800);
    console.log(`\n===== mat=${mat} =====`);
    const html = await p.evaluate(() => {
      const c = document.querySelector(".person-card");
      return c ? c.outerHTML.replace(/\s+/g, " ").slice(0, 900) : "(no card)";
    });
    console.log("card html:", html);
    const counts = await p.evaluate(() => ({
      miniBtn: document.querySelectorAll(".person-card .mini-btn").length,
      nameRow: document.querySelectorAll(".person-name-row").length,
      mergeBtn: document.querySelectorAll(".merge-btn").length,
      info: document.querySelectorAll(".person-info").length,
      miniBtnStyle: (() => { const el = document.querySelector(".person-card .mini-btn"); if (!el) return null;
        const cs = getComputedStyle(el); return { pe: cs.pointerEvents, opacity: cs.opacity, display: cs.display, w: el.getBoundingClientRect().width }; })(),
    }));
    console.log("counts:", JSON.stringify(counts));
    // 悬停卡片再查
    await p.locator(".person-card").first().hover();
    await p.waitForTimeout(300);
    const hover = await p.evaluate(() => ({
      miniBtn: document.querySelectorAll(".person-card .mini-btn").length,
      pe: (() => { const el = document.querySelector(".person-card .mini-btn"); return el ? getComputedStyle(el).pointerEvents : null; })(),
    }));
    console.log("after hover:", JSON.stringify(hover));
    // 直接点卡片内所有 mini-btn
    const btns = await p.locator(".person-card .mini-btn").count();
    console.log("mini-btn count(playwright):", btns);
    if (btns) {
      for (let i = 0; i < btns; i++) {
        const t = await p.locator(".person-card .mini-btn").nth(i).getAttribute("title");
        const txt = await p.locator(".person-card .mini-btn").nth(i).innerText();
        console.log(`  btn[${i}] title=${t} text=${txt}`);
      }
    }
  }
  await b.close();
})();
