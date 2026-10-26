const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";

const USER = {
  id: 1,
  username: "tester",
  email: "t@example.com",
  phone: "13800000000",
  created_at: 1700000000,
  avatar: null,
};

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + String(e).split("\n")[0]));
  p.on("console", (m) => {
    if (m.type() === "error") errs.push("console: " + m.text().slice(0, 240));
  });

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
    window.__TAURI_INTERNALS__ = {
      invoke: stub,
      transformCallback: (cb) => cb,
      convertFileSrc: (x) => x,
      metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
    };
    window.__TAURI__ = {
      core: { invoke: stub, convertFileSrc: (x) => x },
      event: { listen: async () => () => {}, emit: async () => {} },
      dialog: { open: async () => null },
    };
  }, USER);

  await p.goto(`http://localhost:${PORT}/home`, { waitUntil: "load" });
  await p.waitForTimeout(2500);
  console.log("=== url after load:", await p.evaluate(() => location.href));
  console.log("=== body class:", await p.evaluate(() => document.body.className));

  // 打开主题弹窗（🎨）
  const themeBtn = p.locator('button[title="主题"], button[aria-label="主题"]').first();
  const themeBtnAlt = p.locator(".icon-btn").nth(0);
  const count = await p.locator('button[title="主题"]').count();
  console.log("=== theme button count:", count);
  if (count > 0) {
    await p.locator('button[title="主题"]').first().click();
  } else {
    await p.locator(".icon-btn").first().click();
  }
  await p.waitForTimeout(600);
  const dialogOpen = await p.locator(".pm-dialog").count();
  console.log("=== pm-dialog count:", dialogOpen);

  // 切到自定义页签
  const tabTexts = await p.locator(".pm-tab").allInnerTexts();
  console.log("=== tabs:", JSON.stringify(tabTexts));
  const customTab = p.locator(".pm-tab", { hasText: "自定义" }).first();
  if (await customTab.count()) {
    await customTab.click();
    await p.waitForTimeout(400);
  }

  const readCards = () =>
    p.evaluate(() => {
      const cards = [...document.querySelectorAll(".glass-card, .glass-surface")].slice(0, 6);
      return {
        bodyClass: document.body.className,
        material: JSON.parse(localStorage.getItem("pm-theme") || "{}").material,
        widgetMaterial: JSON.parse(localStorage.getItem("pm-theme") || "{}").widgetMaterial,
        cards: cards.map((el) => {
          const cs = getComputedStyle(el);
          const a = getComputedStyle(el, "::after");
          return {
            cls: el.className.slice(0, 60),
            bf: cs.backdropFilter,
            bgImgN: (cs.backgroundImage.match(/gradient/g) || []).length,
            afterContent: a.content,
            afterDisplay: a.display,
            afterBg: a.backgroundImage.slice(0, 40),
          };
        }),
      };
    });

  console.log("=== BEFORE switch:");
  console.log(JSON.stringify(await readCards(), null, 2));

  const liquidBtn = p.locator(".pm-seg button", { hasText: "液态玻璃" }).first();
  console.log("=== liquidBtn count:", await liquidBtn.count());
  if (await liquidBtn.count()) {
    await liquidBtn.click();
    await p.waitForTimeout(700);
    console.log("=== AFTER switch to liquid:");
    console.log(JSON.stringify(await readCards(), null, 2));
    await p.screenshot({ path: "design/preview/_material-after-liquid.png" });

    const plainBtn = p.locator(".pm-seg button", { hasText: "普通卡片" }).first();
    await plainBtn.click();
    await p.waitForTimeout(700);
    console.log("=== AFTER switch back to plain:");
    console.log(JSON.stringify(await readCards(), null, 2));
  }

  console.log("=== errors");
  console.log(errs.slice(0, 15).join("\n") || "(none)");
  await b.close();
})();
