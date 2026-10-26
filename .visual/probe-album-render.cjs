const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };
const ALBUMS = Array.from({ length: 6 }, (_, i) => ({
  id: i + 1, name: "相册" + i, path: "C:/photos/album" + i, cover: null,
  photo_count: 5 * i, created_at: 1700000000, updated_at: 1700000000,
  location: "", tags: [], hidden: false, is_virtual: false,
}));

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  const errs = [];
  p.on("pageerror", (e) => errs.push(String(e).split("\n")[0].slice(0, 120)));
  p.on("console", (m) => { if (m.type() === "error" && !/local resource/.test(m.text())) errs.push(m.text().slice(0, 120)); });
  await p.addInitScript(({ user, albums }) => {
    const stub = async (cmd) => {
      if (cmd === "get_current_user") return user;
      if (cmd === "get_albums") return albums;
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

  await p.goto(`http://localhost:${PORT}/albums`, { waitUntil: "load" });
  await p.waitForTimeout(2500);
  console.log("album-card:", await p.locator(".album-card").count());
  console.log("glass-surface:", await p.locator(".glass-surface").count());
  console.log("body:", await p.evaluate(() => document.body.className));
  const dump = await p.evaluate(() => {
    const el = document.querySelector(".album-card");
    if (!el) return { none: true, html: document.body.innerText.slice(0, 300) };
    const cs = getComputedStyle(el);
    return { cls: el.className, bg: cs.backgroundColor, bf: cs.backdropFilter, w: el.getBoundingClientRect().width };
  });
  console.log("dump:", JSON.stringify(dump));
  console.log("errors:", errs.slice(0, 5).join(" | ") || "(none)");
  await b.close();
})();
