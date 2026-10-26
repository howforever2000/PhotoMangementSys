const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  await p.addInitScript((user) => {
    const stub = async (cmd) => {
      if (cmd === "get_current_user") return user;
      if (/^(get_albums|list_albums|get_albums_with_stats|list_timeline|list_persons|list_categories)$/.test(cmd)) return [];
      if (cmd === "get_person_avatars_bulk") return [];
      if (cmd === "category_index_stats") return { indexed: 0, stale: 0, known: 0, model: "", semantic_categories: 0 };
      return null;
    };
    window.__TAURI_INTERNALS__ = { invoke: stub, transformCallback: (cb) => cb, convertFileSrc: (x) => x,
      metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } } };
    window.__TAURI__ = { core: { invoke: stub, convertFileSrc: (x) => x },
      event: { listen: async () => () => {}, emit: async () => {} }, dialog: { open: async () => null } };
  }, USER);

  await p.goto(`http://localhost:${PORT}/home`, { waitUntil: "load" });
  await p.waitForTimeout(2000);
  await p.locator(".icon-btn").first().click();
  await p.waitForTimeout(500);
  await p.locator(".pm-tab", { hasText: "自定义" }).first().click();
  await p.waitForTimeout(400);

  const rect = await p.locator(".pm-dialog").boundingBox();
  console.log("dialog rect:", JSON.stringify(rect));

  await p.locator(".pm-seg button", { hasText: "普通卡片" }).first().click();
  await p.waitForTimeout(700);
  const a = await p.screenshot({ clip: rect });
  await p.locator(".pm-seg button", { hasText: "液态玻璃" }).first().click();
  await p.waitForTimeout(700);
  const bb = await p.screenshot({ clip: rect });

  const d = await p.evaluate(async ([x, y]) => {
    const load = (s) => new Promise((r) => { const i = new Image(); i.onload = () => r(i); i.src = "data:image/png;base64," + s; });
    const [ia, ib] = await Promise.all([load(x), load(y)]);
    const c = document.createElement("canvas"); c.width = ia.width; c.height = ia.height;
    const g = c.getContext("2d"); g.drawImage(ia, 0, 0);
    const da = g.getImageData(0, 0, c.width, c.height).data;
    g.clearRect(0, 0, c.width, c.height); g.drawImage(ib, 0, 0);
    const db = g.getImageData(0, 0, c.width, c.height).data;
    let n = 0, maxD = 0;
    for (let i = 0; i < da.length; i += 4) {
      const dd = Math.abs(da[i]-db[i]) + Math.abs(da[i+1]-db[i+1]) + Math.abs(da[i+2]-db[i+2]);
      if (dd > 8) n++; if (dd > maxD) maxD = dd;
    }
    return { w: c.width, h: c.height, diffPx: n, total: da.length/4, pct: ((n/(da.length/4))*100).toFixed(2), maxD };
  }, [a.toString("base64"), bb.toString("base64")]);
  console.log("=== 弹窗区域 plain→liquid 像素差:", JSON.stringify(d));

  // 只看弹窗面板（排除按钮高亮区域）—— 取面板空白处一条带
  const strip = { x: rect.x + 20, y: rect.y + 300, width: rect.width - 40, height: 60 };
  await p.locator(".pm-seg button", { hasText: "普通卡片" }).first().click();
  await p.waitForTimeout(600);
  const s1 = await p.screenshot({ clip: strip });
  await p.locator(".pm-seg button", { hasText: "液态玻璃" }).first().click();
  await p.waitForTimeout(600);
  const s2 = await p.screenshot({ clip: strip });
  const d2 = await p.evaluate(async ([x, y]) => {
    const load = (s) => new Promise((r) => { const i = new Image(); i.onload = () => r(i); i.src = "data:image/png;base64," + s; });
    const [ia, ib] = await Promise.all([load(x), load(y)]);
    const c = document.createElement("canvas"); c.width = ia.width; c.height = ia.height;
    const g = c.getContext("2d"); g.drawImage(ia, 0, 0);
    const da = g.getImageData(0, 0, c.width, c.height).data;
    g.clearRect(0, 0, c.width, c.height); g.drawImage(ib, 0, 0);
    const db = g.getImageData(0, 0, c.width, c.height).data;
    let n = 0; for (let i = 0; i < da.length; i += 4) if (Math.abs(da[i]-db[i])+Math.abs(da[i+1]-db[i+1])+Math.abs(da[i+2]-db[i+2]) > 8) n++;
    return { diffPx: n, total: da.length/4, pct: ((n/(da.length/4))*100).toFixed(2) };
  }, [s1.toString("base64"), s2.toString("base64")]);
  console.log("=== 弹窗面板空白带 plain→liquid 像素差:", JSON.stringify(d2));

  // 弹窗自身的液态专属视觉层有没有？
  const detail = await p.evaluate(() => {
    const d = document.querySelector(".pm-dialog");
    const cs = getComputedStyle(d);
    const before = getComputedStyle(d, "::before");
    const after = getComputedStyle(d, "::after");
    return {
      cls: d.className,
      backgroundImage: cs.backgroundImage.slice(0, 120),
      backgroundColor: cs.backgroundColor,
      backdropFilter: cs.backdropFilter,
      boxShadow: cs.boxShadow.slice(0, 80),
      hasGlassCardClass: d.classList.contains("glass-card") || d.classList.contains("glass-surface"),
      afterContent: after.content,
      beforeContent: before.content,
      dialogFillAlphaNote: "dialogFill 来自 compAlpha",
      compAlpha: JSON.parse(localStorage.getItem("pm-theme") || "{}").compAlpha,
    };
  });
  console.log("=== 弹窗计算样式:", JSON.stringify(detail, null, 2));

  await b.close();
})();
