const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + String(e).split("\n")[0]));
  p.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 200)); });

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

  async function setMaterial(m) {
    await p.evaluate((mm) => {
      const raw = JSON.parse(localStorage.getItem("pm-theme") || "{}");
      raw.material = mm;
      localStorage.setItem("pm-theme", JSON.stringify(raw));
    }, m);
    await p.reload({ waitUntil: "load" });
    await p.waitForTimeout(2200);
  }

  async function clickProbe(label, selector) {
    const loc = p.locator(selector).first();
    const cnt = await p.locator(selector).count();
    if (!cnt) return console.log(`  ${label}: (元素不存在)`);
    const box = await loc.boundingBox();
    const hit = await p.evaluate(([sel, x, y]) => {
      const el = document.elementFromPoint(x, y);
      const target = document.querySelector(sel);
      return {
        hitTag: el ? `${el.tagName.toLowerCase()}.${(el.className || "").toString().split(" ").filter(Boolean).slice(0, 3).join(".")}` : null,
        isTargetOrInside: !!el && !!target && (target === el || target.contains(el)),
        hitPointerEvents: el ? getComputedStyle(el).pointerEvents : null,
        targetPE: target ? getComputedStyle(target).pointerEvents : null,
      };
    }, [selector, box.x + box.width / 2, box.y + box.height / 2]);
    let navigated = null;
    const before = p.url();
    await loc.click({ timeout: 3000 }).catch((e) => console.log(`  ${label}: click 失败 ${String(e).slice(0, 80)}`));
    await p.waitForTimeout(700);
    navigated = p.url() !== before ? p.url() : null;
    console.log(`  ${label}: count=${cnt} box=${Math.round(box.width)}x${Math.round(box.height)} ${JSON.stringify(hit)} nav=${navigated}`);
  }

  for (const mat of ["plain", "liquid"]) {
    console.log(`\n========== material = ${mat} ==========`);
    await p.goto(`http://localhost:${PORT}/home`, { waitUntil: "load" });
    await p.waitForTimeout(1500);
    await setMaterial(mat);
    console.log("  body:", await p.evaluate(() => document.body.className));

    // ⚙ 设置菜单
    await clickProbe("⚙ 设置按钮", '.settings-wrap .icon-btn');
    const menuOpen = await p.locator(".settings-menu").count();
    console.log("    → settings-menu 出现:", menuOpen);
    if (menuOpen) {
      await p.locator(".settings-mask").click({ position: { x: 10, y: 10 } }).catch(() => {});
      await p.waitForTimeout(300);
      console.log("    → 点遮罩后 menu:", await p.locator(".settings-menu").count());
    }

    // 🎨 主题按钮
    await clickProbe("🎨 主题按钮", '.user-box .icon-btn');
    console.log("    → pm-dialog 出现:", await p.locator(".pm-dialog").count());
    await p.keyboard.press("Escape");
    await p.waitForTimeout(300);

    // 模块卡
    await clickProbe("模块卡（相册管理）", '.module-card');
    if (p.url().includes("/albums")) {
      await p.goto(`http://localhost:${PORT}/home`, { waitUntil: "load" });
      await p.waitForTimeout(1200);
    }
    console.log("  errors:", errs.slice(0, 3).join(" | ") || "(none)");
  }

  await b.close();
})();
