const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };

const PERSONS = [
  { id: "p01", name: "张三", face_count: 128, created_at: "2024-01-02" },
  { id: "p02", name: "李四", face_count: 96, created_at: "2024-02-11" },
  { id: "p03", name: "王五", face_count: 74, created_at: "2024-03-05" },
  { id: "p04", name: "赵六", face_count: 51, created_at: "2024-04-18" },
  { id: "p05", name: "钱七", face_count: 33, created_at: "2024-05-22" },
  { id: "p06", name: "孙八", face_count: 21, created_at: "2024-06-30" },
];

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + String(e).split("\n")[0]));
  p.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 160)); });

  await p.addInitScript(({ user, persons }) => {
    const stub = async (cmd) => {
      if (cmd === "get_current_user") return user;
      if (cmd === "get_albums" || cmd === "list_albums" || cmd === "get_albums_with_stats") return [];
      if (cmd === "list_timeline" || cmd === "list_categories") return [];
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

  const setMat = async (m, wm) => {
    await p.evaluate(([mm, ww]) => {
      const raw = JSON.parse(localStorage.getItem("pm-theme") || "{}");
      raw.material = mm; raw.widgetMaterial = ww;
      localStorage.setItem("pm-theme", JSON.stringify(raw));
    }, [m, wm]);
  };
  const shot = async () => (await p.screenshot({ type: "png" })).toString("base64");
  const diff = (a64, b64) => p.evaluate(async ([a, b]) => {
    const load = (d) => new Promise((r) => { const i = new Image(); i.onload = () => r(i); i.src = "data:image/png;base64," + d; });
    const [ia, ib] = await Promise.all([load(a), load(b)]);
    const c = document.createElement("canvas"); c.width = ia.width; c.height = ia.height;
    const g = c.getContext("2d"); g.drawImage(ia, 0, 0);
    const da = g.getImageData(0, 0, c.width, c.height).data;
    g.clearRect(0, 0, c.width, c.height); g.drawImage(ib, 0, 0);
    const db = g.getImageData(0, 0, c.width, c.height).data;
    let n = 0; for (let i = 0; i < da.length; i += 4) {
      if (Math.abs(da[i] - db[i]) + Math.abs(da[i+1] - db[i+1]) + Math.abs(da[i+2] - db[i+2]) > 8) n++;
    }
    return { pct: ((n / (da.length / 4)) * 100).toFixed(2), diffPx: n };
  }, [a64, b64]);

  for (const [mat, route, label] of [
    ["plain", "/smart?tab=face", "人物界面"],
    ["liquid", "/smart?tab=face", "人物界面"],
    ["plain", "/smart?tab=overview", "全局概览(墙=glass)"],
    ["plain", "/smart?tab=overview", "全局概览(墙=plain)"],
  ]) {
    const wm = label.includes("墙=plain") ? "plain" : "glass";
    await p.goto(`http://localhost:${PORT}${route}`, { waitUntil: "load" });
    await p.waitForTimeout(800);
    await setMat(mat, wm);
    await p.reload({ waitUntil: "load" });
    await p.waitForTimeout(2000);
    const body = await p.evaluate(() => document.body.className);
    const info = await p.evaluate(() => {
      const q = (s) => document.querySelector(s);
      const g = (el) => { if (!el) return null; const cs = getComputedStyle(el);
        return { bg: cs.backgroundColor, bf: cs.backdropFilter, imgN: (cs.backgroundImage.match(/gradient/g) || []).length }; };
      const widget = q(".ui-widget");
      return {
        personCard: g(q(".person-card")),
        widgetCls: widget ? widget.className : null,
        widget: g(widget),
      };
    });
    const png = await shot();
    const tag = `${label} mat=${mat} wm=${wm}`;
    if (mat === "plain" || wm === "plain") {
      global[`s_${tag}`] = png;
    } else {
      const prev = global[`s_${label} mat=plain wm=${wm}`];
      console.log(`=== ${tag} body=${body}`, JSON.stringify(info));
      if (prev) console.log(`    vs plain 像素差:`, JSON.stringify(await diff(prev, png)));
      global[`s_${tag}`] = png;
    }
  }

  // 再比概览墙 glass vs plain
  const a = global["s_全局概览(墙=glass) mat=plain wm=glass"];
  const c = global["s_全局概览(墙=plain) mat=plain wm=plain"];
  if (a && c) console.log("=== 概览墙 glass vs plain 像素差:", JSON.stringify(await diff(a, c)));

  console.log("errors:", errs.slice(0, 6).join(" | ") || "(none)");
  await b.close();
})();
