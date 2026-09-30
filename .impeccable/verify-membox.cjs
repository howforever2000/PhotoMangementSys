/**
 * 验收：1) Timeline 回到顶部箭头；2) Memories 月度浏览框（只加载该月 + 框内回到顶部 + 灯箱层级）
 * 依赖：本地 vite dev (1420) + ./mock-init.cjs 的 IPC mock（此处覆写为更长的时间线数据）
 */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

/** 生成足够多的照片，保证时间线页可长距离滚动、月份分组够大 */
function bigTimeline() {
  const months = ["2025-09", "2025-08", "2025-07", "2024-11", "2024-02"];
  const rows = [];
  let id = 1000;
  for (const m of months) {
    const n = m === "2025-09" ? 96 : 40;
    for (let i = 0; i < n; i++) {
      const day = String((i % 28) + 1).padStart(2, "0");
      rows.push({
        id: id++,
        path: `D:/Photos/${m}/${m}-${i}.jpg`,
        parent_dir: `D:/Photos/${m}`,
        album_id: 1,
        album_name: "京都秋色",
        album_path: "D:/Photos/京都秋色",
        content: `照片 ${i}`,
        category: "life",
        sub_category: null,
        label: `照片 ${i}`,
        confidence: 0.9,
        person_ids: [],
        shoot_time: `${m}-${day} 12:0${i % 10}:00`,
        location: "京都",
        iso: "400", aperture: "f/1.8", shutter_speed: "1/125", focal_length: "35",
      });
    }
  }
  return rows;
}

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 900 } });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push("pageerror: " + e.message));
  page.on("console", (m) => {
    if (m.type() === "error") errors.push("console: " + m.text());
  });

  await page.addInitScript(INIT);
  // 统计 get_photo_thumbs 请求量（验证缩略图按需加载而非全量）
  await page.addInitScript(() => {
    const core = window.__TAURI_INTERNALS__;
    const orig = core.invoke.bind(core);
    window.__THUMB_REQ__ = [];
    core.invoke = (cmd, payload) => {
      if (cmd === "get_photo_thumbs") window.__THUMB_REQ__.push((payload && payload.paths ? payload.paths : []).length);
      return orig(cmd, payload);
    };
  });
  await page.addInitScript((rows) => {
    window.__MOCK_TIMELINE__ = rows;
  }, bigTimeline());

  /* ---------- 1) 时间线：回到顶部 ---------- */
  await page.goto("http://localhost:1420/timeline", { waitUntil: "domcontentloaded", timeout: 20000 });
  await page.waitForSelector(".tl-card", { timeout: 15000 });
  await page.waitForTimeout(1200);
  const before = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.evaluate(() => window.scrollTo(0, 1500));
  await page.waitForTimeout(500);
  const btnShown = await page.$eval(".tl-top-btn", (el) => getComputedStyle(el).display !== "none").catch(() => false);
  const btnGeo = await page.$eval(".tl-top-btn", (el) => {
    const r = el.getBoundingClientRect();
    const cs = getComputedStyle(el);
    return { right: Math.round(window.innerWidth - r.right), bottom: Math.round(window.innerHeight - r.bottom), w: Math.round(r.width), h: Math.round(r.height), pos: cs.position, z: cs.zIndex };
  }).catch((e) => ({ err: String(e) }));
  const y1 = await page.evaluate(() => window.scrollY);
  await page.screenshot({ path: ".impeccable/review/timeline-topbtn.png" });
  await page.click(".tl-top-btn");
  await page.waitForTimeout(1200);
  const y2 = await page.evaluate(() => window.scrollY);
  console.log(`[timeline] scrollHeight=${before} scrolledTo=${y1} btnShown=${btnShown} afterClick=${y2} geo=${JSON.stringify(btnGeo)}`);

  /* ---------- 2) 回忆：月度浏览框 ---------- */
  await page.goto("http://localhost:1420/memories", { waitUntil: "domcontentloaded", timeout: 20000 });
  await page.waitForSelector(".mem-story", { timeout: 15000 });
  await page.waitForTimeout(1200);
  const mountThumbs = await page.evaluate(() => { const s = (window.__THUMB_REQ__ || []).reduce((a, b) => a + b, 0); window.__THUMB_REQ__ = []; return s; });
  const totalRows = await page.evaluate(() => window.__MOCK_TIMELINE__.length);
  const storyLabel = await page.$eval(".mem-story .mem-story-title", (el) => el.textContent.trim());
  const storyMeta = await page.$eval(".mem-story .mem-story-meta", (el) => el.textContent.trim());
  await page.click(".mem-story");
  await page.waitForSelector(".mb-mask", { timeout: 8000 });
  await page.waitForTimeout(1500);
  const head = await page.$eval(".mb-head", (el) => el.textContent.replace(/\s+/g, " ").trim());
  const cellCount = await page.$$eval(".mb-cell", (els) => els.length);
  const urlAfterOpen = page.url();
  const bodyOverflow = await page.evaluate(() => document.body.style.overflow);
  const boxThumbs = await page.evaluate(() => (window.__THUMB_REQ__ || []).reduce((a, b) => a + b, 0));
  await page.screenshot({ path: ".impeccable/review/mem-monthbox.png" });

  // 框内滚动 → 回到顶部箭头
  await page.$eval(".mb-scroll", (el) => el.scrollTo(0, 600));
  await page.waitForTimeout(400);
  const mbTopShown = await page.$eval(".mb-top-btn", (el) => getComputedStyle(el).display !== "none").catch(() => false);
  await page.screenshot({ path: ".impeccable/review/mem-monthbox-scrolled.png" });
  await page.click(".mb-top-btn");
  await page.waitForTimeout(900);
  const mbScrollTop = await page.$eval(".mb-scroll", (el) => el.scrollTop);
  console.log(`[memories] story=${storyLabel} (${storyMeta}) cells=${cellCount} url=${urlAfterOpen} bodyOverflow='${bodyOverflow}'`);
  console.log(`[memories] thumbPaths: mount=${mountThumbs} (totalRows=${totalRows}) monthBox=${boxThumbs}`);
  console.log(`[memories] head="${head}" mbTopBtn=${mbTopShown} scrollTopAfter=${mbScrollTop}`);

  // 点照片 → 灯箱应在浏览框之上（z-index 1000 > 950）
  await page.click(".mb-cell");
  await page.waitForSelector(".lb-overlay", { timeout: 8000 });
  await page.waitForTimeout(700);
  const hit = await page.evaluate(() => {
    const el = document.elementFromPoint(window.innerWidth / 2, window.innerHeight / 2);
    return !!el && !!el.closest(".lb-overlay");
  });
  await page.screenshot({ path: ".impeccable/review/mem-monthbox-lightbox.png" });
  // Esc 关灯箱，浏览框仍在
  await page.keyboard.press("Escape");
  await page.waitForTimeout(400);
  const lbGone = (await page.$(".lb-overlay")) === null;
  const maskStill = (await page.$(".mb-mask")) !== null;
  // Esc 关浏览框
  await page.keyboard.press("Escape");
  await page.waitForTimeout(400);
  const maskGone = (await page.$(".mb-mask")) === null;
  const overflowRestored = await page.evaluate(() => document.body.style.overflow);
  console.log(`[memories] lightboxOnTop=${hit} lbClosedByEsc=${lbGone} maskKept=${maskStill} maskClosedByEsc=${maskGone} overflow='${overflowRestored}' url=${page.url()}`);

  console.log("ERRORS:", errors.length ? errors.slice(0, 8).join("\n") : "(none)");
  await browser.close();
})();
