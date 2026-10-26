const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const USER = { id: 1, username: "tester", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };
const PERSONS = Array.from({ length: 6 }, (_, i) => ({ id: "p" + i, name: "人物" + i, face_count: 10 * i, created_at: "2024-01-02" }));

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

  await p.goto(`http://localhost:${PORT}/smart?tab=face`, { waitUntil: "load" });
  await p.waitForTimeout(600);
  await p.evaluate(() => { const r = JSON.parse(localStorage.getItem("pm-theme") || "{}"); r.material = "liquid"; localStorage.setItem("pm-theme", JSON.stringify(r)); });
  await p.reload({ waitUntil: "load" });
  await p.waitForTimeout(1800);

  const state = () => p.evaluate(() => {
    const c = document.querySelector(".person-card");
    const a = getComputedStyle(c, "::after");
    const hit = document.elementFromPoint(
      c.getBoundingClientRect().left + 10, c.getBoundingClientRect().top + 10);
    return { afterContent: a.content, afterDisplay: a.display, afterBg: a.backgroundImage.slice(0, 45),
      afterPE: a.pointerEvents, topAtCardCorner: hit ? hit.tagName.toLowerCase() + "." + String(hit.className).split(" ")[0] : null };
  });
  console.log("① 现状:", JSON.stringify(await state()));

  // 注入 A：补回基础 ::after 但【漏写】pointer-events:none —— 复现历史"点击被吞"
  await p.addStyleTag({ content: `body.mat-liquid .glass-card::after{content:"";position:absolute;inset:0;border-radius:inherit;}` });
  await p.waitForTimeout(400);
  console.log("② 补 content 但无 pointer-events:none:", JSON.stringify(await state()));
  let swallowed = null;
  await p.locator(".person-avatar-wrap").first().click({ timeout: 2500 }).catch((e) => { swallowed = String(e).split("\n")[0].slice(0, 80); });
  await p.waitForTimeout(500);
  console.log("   → 点头像后 viewer:", await p.locator(".viewer-mask").count(), swallowed ? "| 点击失败: " + swallowed : "");

  // 注入 B：正确写法（含 pointer-events:none + z-index）
  await p.addStyleTag({ content: `body.mat-liquid .glass-card::after{pointer-events:none;z-index:1;}` });
  await p.waitForTimeout(300);
  console.log("③ 补 pointer-events:none 后:", JSON.stringify(await state()));
  if (await p.locator(".viewer-mask").count()) {
    await p.keyboard.press("Escape"); await p.waitForTimeout(400);
    if (await p.locator(".viewer-mask").count()) { await p.locator(".viewer-mask").click({ position: { x: 6, y: 6 } }); await p.waitForTimeout(400); }
  }
  await p.locator(".person-avatar-wrap").first().click({ timeout: 2500 }).catch(() => {});
  await p.waitForTimeout(500);
  console.log("   → 点头像后 viewer:", await p.locator(".viewer-mask").count());

  // 棱环渲染后像素差
  const shot = async () => (await p.screenshot({ clip: { x: 100, y: 200, width: 900, height: 420 } })).toString("base64");
  const diff = (x, y) => p.evaluate(async ([a, c]) => {
    const load = (s) => new Promise((r) => { const i = new Image(); i.onload = () => r(i); i.src = "data:image/png;base64," + s; });
    const [ia, ib] = await Promise.all([load(a), load(c)]);
    const cv = document.createElement("canvas"); cv.width = ia.width; cv.height = ia.height;
    const g = cv.getContext("2d"); g.drawImage(ia, 0, 0);
    const da = g.getImageData(0, 0, cv.width, cv.height).data;
    g.clearRect(0, 0, cv.width, cv.height); g.drawImage(ib, 0, 0);
    const db = g.getImageData(0, 0, cv.width, cv.height).data;
    let n = 0; for (let i = 0; i < da.length; i += 4) if (Math.abs(da[i]-db[i])+Math.abs(da[i+1]-db[i+1])+Math.abs(da[i+2]-db[i+2]) > 8) n++;
    return +(((n/(da.length/4))*100).toFixed(2));
  }, [a, c]);

  // 去掉注入看基线，再对比
  await p.reload({ waitUntil: "load" });
  await p.waitForTimeout(1800);
  const noRim = await shot();
  await p.addStyleTag({ content: `body.mat-liquid .glass-card::after{content:"";position:absolute;inset:0;border-radius:inherit;pointer-events:none;z-index:1;}` });
  await p.waitForTimeout(500);
  const withRim = await shot();
  console.log("④ 棱环渲染前后像素差:", await diff(noRim, withRim), "% (卡片区域)");

  await b.close();
})();
