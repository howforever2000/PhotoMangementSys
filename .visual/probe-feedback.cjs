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

  // 采样元素的「可感知反馈」：hover 前后 background / transform / boxShadow / opacity 差异
  async function feedback(label, sel) {
    const loc = p.locator(sel).first();
    if (!(await loc.count())) return console.log(`  ${label}: 不存在`);
    const snap = () => loc.evaluate((el) => {
      const cs = getComputedStyle(el);
      return { bg: cs.backgroundColor, bgi: cs.backgroundImage.slice(0, 40), tr: cs.transform, sh: cs.boxShadow.slice(0, 40), op: cs.opacity, cursor: cs.cursor };
    });
    const base = await snap();
    await loc.hover({ timeout: 2000 }).catch(() => {});
    await p.waitForTimeout(400);
    const hov = await snap();
    await p.mouse.down(); await p.waitForTimeout(200);
    const act = await snap();
    await p.mouse.up();
    await p.mouse.move(5, 5); await p.waitForTimeout(300);
    const changed = [];
    for (const k of ["bg", "tr", "sh", "op", "bgi"]) if (base[k] !== hov[k]) changed.push(`hover.${k}: ${base[k]} → ${hov[k]}`);
    for (const k of ["bg", "tr", "sh", "op"]) if (hov[k] !== act[k]) changed.push(`active.${k}: ${hov[k]} → ${act[k]}`);
    console.log(`  ${label} cursor=${base.cursor} | ${changed.length ? changed.join(" ; ") : "❌ 无任何 hover/active 视觉反馈"}`);
  }

  for (const mat of ["plain", "liquid"]) {
    console.log(`\n===== mat=${mat} 交互反馈 =====`);
    await p.goto(`http://localhost:${PORT}/smart?tab=face`, { waitUntil: "load" });
    await p.waitForTimeout(500);
    await p.evaluate((m) => { const r = JSON.parse(localStorage.getItem("pm-theme") || "{}"); r.material = m; localStorage.setItem("pm-theme", JSON.stringify(r)); }, mat);
    await p.reload({ waitUntil: "load" });
    await p.waitForTimeout(1700);
    await feedback("人物卡 .person-card", ".person-card");
    await feedback("头像 .person-avatar-wrap", ".person-avatar-wrap");
    await feedback("tab 按钮 .tab", ".tab");
    await feedback("顶部导航 a", "header a");
    await feedback("图标按钮 .icon-btn", ".icon-btn");

    await p.goto(`http://localhost:${PORT}/smart?tab=overview`, { waitUntil: "load" });
    await p.waitForTimeout(2000);
    await feedback("widget 玻璃卡 .ui-widget", ".ui-widget");
    await feedback("widget 链接卡 .ui-widget--link", ".ui-widget--link");
    await feedback("详情按钮 .ui-widget-link", ".ui-widget-link");
  }

  await b.close();
})();
