/**
 * FEAT-123 验收探针：全局概览快照
 *
 * 跑法：npx vite --port 1519  →  node .visual/probe-overview-snapshot.cjs
 *
 * 断言：
 *   ① 概览只调 1 次 overview_stats，**不再调 list_timeline / category_index_stats**
 *      （改前：list_timeline 14,438 KB + category_index_stats 300~570 ms）
 *   ② 页面无报错，hero 读数渲染出来（总张数/年份跨度）
 *   ③ 切走再切回：store 缓存命中，overview_stats 不再被调用（0 新增）
 */
const { chromium } = require("playwright");

const PORT = process.env.VITE_PORT || 1519;
const BASE = `http://localhost:${PORT}/.visual/preview.html#/smart`;

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });

  await page.goto(BASE, { waitUntil: "load" });
  await page.waitForSelector(".ov-hero", { timeout: 20000 }).catch(() => {});
  await page.waitForTimeout(1500);

  const calls1 = await page.evaluate(() => ({ ...window.__PM_MOCK_CALLS__ }));
  const t1 = await page.evaluate(() => ({
    hero: document.querySelector(".ov-hero")?.innerText?.replace(/\s+/g, " ").slice(0, 120) ?? "",
    widgets: document.querySelectorAll("[class*='w-']").length,
    years: document.body.innerText.match(/\d{4} – \d{4}/)?.[0] ?? "",
  }));
  console.log("首次进入:", t1);
  console.log("首次请求:", calls1);

  // 切走再切回（走同一路由的另一个 tab 再回来）
  await page.goto(`http://localhost:${PORT}/.visual/preview.html#/albums`, { waitUntil: "load" });
  await page.waitForTimeout(300);
  await page.goto(BASE, { waitUntil: "load" });
  await page.waitForTimeout(1200);
  const calls2 = await page.evaluate(() => ({ ...window.__PM_MOCK_CALLS__ }));

  const delta = {};
  for (const k of Object.keys(calls2)) {
    const d = (calls2[k] ?? 0) - (calls1[k] ?? 0);
    if (d > 0) delta[k] = d;
  }
  console.log("切回后新增请求:", delta);

  const problems = [];
  if ((calls1.overview_stats ?? 0) !== 1) problems.push(`overview_stats 调了 ${calls1.overview_stats} 次（期望 1）`);
  if ((calls1.list_timeline ?? 0) > 0) problems.push(`仍在调 list_timeline（${calls1.list_timeline} 次）`);
  if ((calls1.category_index_stats ?? 0) > 0)
    problems.push(`仍在调 category_index_stats（${calls1.category_index_stats} 次）`);
  if (!t1.hero) problems.push("hero 读数没渲染出来");
  if ((delta.overview_stats ?? 0) > 0)
    problems.push(`切回后又重拉了一次 overview_stats（缓存没生效）`);
  if (errors.length) problems.push(`页面报错: ${errors.slice(0, 3).join(" | ")}`);

  console.log("\n结果:", problems.length ? `❌ ${problems.length} 项不达标` : "✅ 全部达标");
  for (const p of problems) console.log("  -", p);

  await browser.close();
  process.exit(problems.length ? 1 : 0);
})();
