const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = {
  id: 1, username: "tester", email: "t@example.com", phone: "13800000000",
  created_at: 1700000000, avatar: null,
};

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  await p.addInitScript((user) => {
    const stub = async (cmd) => {
      if (cmd === "get_current_user") return user;
      if (cmd === "get_albums" || cmd === "list_albums" || cmd === "get_albums_with_stats") return [];
      if (cmd === "list_timeline" || cmd === "list_persons") return [];
      if (cmd === "list_categories") return [];
      if (cmd === "get_person_avatars_bulk") return [];
      if (cmd === "category_index_stats") return { indexed: 0, stale: 0, known: 0, model: "", semantic_categories: 0 };
      return null;
    };
    window.__TAURI_INTERNALS__ = { invoke: stub, transformCallback: (cb) => cb, convertFileSrc: (x) => x,
      metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } } };
    window.__TAURI__ = { core: { invoke: stub, convertFileSrc: (x) => x },
      event: { listen: async () => () => {}, emit: async () => {} }, dialog: { open: async () => null } };
  }, USER);

  async function shot64() {
    return (await p.screenshot({ type: "png" })).toString("base64");
  }

  // 在页面里用 canvas 比对两张 base64 PNG
  async function diff64(a64, b64) {
    return p.evaluate(async ([a, b]) => {
      const load = (d) => new Promise((res) => { const i = new Image(); i.onload = () => res(i); i.src = "data:image/png;base64," + d; });
      const [ia, ib] = await Promise.all([load(a), load(b)]);
      const c = document.createElement("canvas");
      c.width = ia.width; c.height = ia.height;
      const g = c.getContext("2d");
      g.drawImage(ia, 0, 0);
      const da = g.getImageData(0, 0, c.width, c.height).data;
      g.clearRect(0, 0, c.width, c.height);
      g.drawImage(ib, 0, 0);
      const db = g.getImageData(0, 0, c.width, c.height).data;
      let n = 0, maxDelta = 0;
      for (let i = 0; i < da.length; i += 4) {
        const d = Math.abs(da[i] - db[i]) + Math.abs(da[i + 1] - db[i + 1]) + Math.abs(da[i + 2] - db[i + 2]);
        if (d > 8) n++;
        if (d > maxDelta) maxDelta = d;
      }
      return { diffPixels: n, totalPx: da.length / 4, pct: ((n / (da.length / 4)) * 100).toFixed(3), maxDelta };
    }, [a64, b64]);
  }

  async function openCustomTab() {
    await p.locator(".icon-btn").first().click();
    await p.waitForTimeout(500);
    await p.locator(".pm-tab", { hasText: "自定义" }).first().click();
    await p.waitForTimeout(350);
  }
  async function closeDialog() {
    await p.keyboard.press("Escape");
    await p.waitForTimeout(400);
    if (await p.locator(".pm-dialog").count()) {
      await p.locator(".pm-modal").click({ position: { x: 24, y: 24 } });
      await p.waitForTimeout(400);
    }
  }
  const read = () => p.evaluate(() => {
    const g = (sel) => {
      const el = document.querySelector(sel);
      if (!el) return null;
      const cs = getComputedStyle(el);
      const a = getComputedStyle(el, "::after");
      return {
        bg: cs.backgroundColor,
        imgN: (cs.backgroundImage.match(/gradient/g) || []).length,
        bf: cs.backdropFilter,
        shadow: cs.boxShadow.slice(0, 50),
        afterContent: a.content, afterDisplay: a.display,
      };
    };
    return {
      body: document.body.className,
      mat: JSON.parse(localStorage.getItem("pm-theme") || "{}").material,
      dialog: g(".pm-dialog"),
      moduleCard: g(".module-card"),
      userBox: g(".user-box"),
    };
  });

  await p.goto(`http://localhost:${PORT}/home`, { waitUntil: "load" });
  await p.waitForTimeout(2500);

  // ---------- A. 弹窗自身 + 页面（弹窗开着） ----------
  await openCustomTab();
  await p.locator(".pm-seg button", { hasText: "普通卡片" }).first().click();
  await p.waitForTimeout(700);
  await p.screenshot({ path: "design/preview/_m-plain.png" });
  const a64 = await shot64();
  const plainInfo = await read();

  await p.locator(".pm-seg button", { hasText: "液态玻璃" }).first().click();
  await p.waitForTimeout(700);
  await p.screenshot({ path: "design/preview/_m-liquid.png" });
  const b64 = await shot64();
  const liquidInfo = await read();

  console.log("=== PLAIN state:", JSON.stringify(plainInfo, null, 2));
  console.log("=== LIQUID state:", JSON.stringify(liquidInfo, null, 2));
  console.log("=== 截图 diff（弹窗开着）:", JSON.stringify(await diff64(a64, b64)));

  // ---------- B. 只切卡片材质，页面本体差异 ----------
  await closeDialog();
  await p.waitForTimeout(300);
  await p.screenshot({ path: "design/preview/_page-liquid.png" });
  const pageLiquid64 = await shot64();
  console.log("=== 页面本体（liquid）:", JSON.stringify(await read()));

  await openCustomTab();
  await p.locator(".pm-seg button", { hasText: "普通卡片" }).first().click();
  await p.waitForTimeout(700);
  await closeDialog();
  await p.waitForTimeout(300);
  await p.screenshot({ path: "design/preview/_page-plain.png" });
  const pagePlain64 = await shot64();
  console.log("=== 页面本体（plain）:", JSON.stringify(await read()));
  console.log("=== 截图 diff（页面本体）:", JSON.stringify(await diff64(pagePlain64, pageLiquid64)));

  await b.close();
})();
