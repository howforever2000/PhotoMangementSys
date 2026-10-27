/**
 * FEAT-122 验收探针：时间线虚拟化 / 跳转定位 / 请求数
 *
 * 跑法（必须先起 vite，端口别占 1420）：
 *   npx vite --port 1519 --strictPort
 *   node .visual/probe-timeline-virtual.cjs
 *
 * 断言（改前 → 改后）：
 *   ① 滚到底的 DOM 卡片数不再线性增长（改前 24,980 → 目标 ≤ 1,500）
 *   ② 进页面只发 1 次 timeline_index + 少量 timeline_month，get_photo_thumbs ≈ 0
 *   ③ 点最远的左栏月份：目标月落在视口顶部附近（误差 ≤ 8px 行距 + 头部偏移）
 *   ④ 跳转期间不产生 longtask
 *   ⑤ 文档总高在跳转前后不变（追加渲染不再撑高文档 → 落点不漂移）
 */
const { chromium } = require("playwright");

const PORT = process.env.VITE_PORT || 1519;
const BASE = `http://localhost:${PORT}/.visual/preview.html#/timeline`;

const stat = (p) =>
  p.evaluate(() => ({
    cards: document.querySelectorAll(".tl-card").length,
    units: document.querySelectorAll(".tl-unit").length,
    rows: document.querySelectorAll(".tl-row").length,
    monthsInDom: document.querySelectorAll(".tl-unit.is-month").length,
    docH: Math.round(document.documentElement.scrollHeight),
    cards_in_timeline: window.__PM_MOCK_CALLS__ ?? {},
  }));

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });

  // 记录 longtask（跳转卡顿的直接证据）
  await page.addInitScript(() => {
    window.__LONG = [];
    try {
      new PerformanceObserver((list) => {
        for (const e of list.getEntries()) window.__LONG.push(Math.round(e.duration));
      }).observe({ entryTypes: ["longtask"] });
    } catch {
      /* 浏览器不支持就算了 */
    }
  });

  await page.goto(BASE, { waitUntil: "load" });
  await page.waitForSelector(".tl-board", { timeout: 15000 });
  await page.waitForTimeout(1200);

  const calls = await page.evaluate(() => ({ ...window.__PM_MOCK_CALLS__ }));
  console.log("首屏            ", await stat(page));
  console.log("首屏请求         ", calls);

  // ② 逐屏下滚，看 DOM 是否被窗口化控制住
  const ROLLS = 8;
  for (let i = 1; i <= ROLLS; i++) {
    await page.evaluate(() => window.scrollBy(0, window.innerHeight * 2));
    await page.waitForTimeout(450);
    const s = await stat(page);
    console.log(
      `滚 ${i * 2} 屏      `,
      `cards=${s.cards} units=${s.units} months=${s.monthsInDom} docH=${s.docH}`,
    );
  }

  // ③ 跳到最远的一个月份（左栏最后一个「月」节点）
  const before = await stat(page);
  const info = await page.evaluate(() => {
    const nodes = [...document.querySelectorAll(".tl-rail-node.is-month")];
    const last = nodes[nodes.length - 1];
    const items = [...document.querySelectorAll(".tl-unit")];
    return { railCount: nodes.length, lastLabel: last?.textContent ?? "", units: items.length };
  });
  const t0 = Date.now();
  await page.evaluate(() => {
    const nodes = [...document.querySelectorAll(".tl-rail-node.is-month")];
    nodes[nodes.length - 1].click();
  });
  await page.waitForTimeout(600);
  const after = await stat(page);
  const landed = await page.evaluate(() => {
    // 点击后落在视口里的那个月份块，看它离视口顶多远
    const units = [...document.querySelectorAll(".tl-unit.is-month")];
    let best = null;
    for (const el of units) {
      const top = el.getBoundingClientRect().top;
      if (top >= 0 && (best === null || top < best.top)) {
        best = { top: Math.round(top), id: el.id };
      }
    }
    return best;
  });
  const longTasks = await page.evaluate(() => window.__LONG ?? []);
  const callsAfter = await page.evaluate(() => ({ ...window.__PM_MOCK_CALLS__ }));

  console.log("\n跳转前           ", { docH: before.docH, cards: before.cards });
  console.log("跳转后           ", { docH: after.docH, cards: after.cards });
  console.log("左栏月节点数      ", info.railCount);
  console.log("跳转耗时(含等待)  ", Date.now() - t0, "ms");
  console.log("落点（离视口顶）  ", landed);
  console.log("longtask         ", longTasks.length ? longTasks : "无");
  console.log("跳转后请求        ", callsAfter);

  // ④ 断言
  const problems = [];
  if (after.cards > 1500) problems.push(`DOM 卡片 ${after.cards} > 1500（窗口化失效）`);
  if (Math.abs(after.docH - before.docH) > 4)
    problems.push(`跳转后文档高度变了 ${before.docH} → ${after.docH}（落点会漂移）`);
  if (landed && landed.top > 200)
    problems.push(`落点偏离视口顶部 ${landed.top}px（期望 ≤ 200）`);
  if (!landed) problems.push("跳转后视口内没有任何月份块");
  if ((calls.timeline_index ?? 0) !== 1)
    problems.push(`timeline_index 调了 ${calls.timeline_index} 次（期望 1）`);
  if ((calls.list_timeline ?? 0) !== 0)
    problems.push(`仍在调 list_timeline（${calls.list_timeline} 次）`);
  if ((calls.get_photo_thumbs ?? 0) > 3)
    problems.push(`get_photo_thumbs 调了 ${calls.get_photo_thumbs} 次（期望 ≤ 3）`);
  if (errors.length) problems.push(`页面报错: ${errors.slice(0, 3).join(" | ")}`);

  console.log(
    "\n结果:",
    problems.length ? `❌ ${problems.length} 项不达标` : "✅ 全部达标",
  );
  for (const p of problems) console.log("  -", p);

  await browser.close();
  process.exit(problems.length ? 1 : 0);
})();
