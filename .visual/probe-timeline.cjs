/**
 * 时间线布局探针：验证左栏 + 行式密排 + 滚动跟随
 * node .visual/probe-timeline.cjs [tag]
 */
const { chromium } = require("playwright");
const path = require("path");
const tag = process.argv[2] || "now";

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 2 });
  const errs = [];
  p.on("pageerror", (e) => errs.push(String(e).split("\n")[0]));
  p.on("console", (m) => {
    if (m.type() === "error") errs.push("console: " + m.text().slice(0, 200));
  });

  await p.goto("http://localhost:1420/.visual/timeline-probe.html", { waitUntil: "load" });
  await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 20000 }).catch(() => {});
  await p.waitForTimeout(900);

  /* ── ① 行式密排是否成立：每行宽度是否铺满、行高是否参差 ── */
  const layout = await p.evaluate(() => {
    const board = document.getElementById("tl-board");
    const boardW = board ? Math.round(board.clientWidth) : 0;
    const rows = [...document.querySelectorAll(".tl-row")].map((r) => {
      const cards = [...r.querySelectorAll(".tl-card")];
      const sum = cards.reduce((n, c) => n + c.getBoundingClientRect().width, 0);
      const gap = 8 * (cards.length - 1);
      return {
        n: cards.length,
        h: Math.round(r.getBoundingClientRect().height),
        w: Math.round(sum + gap),
      };
    });
    return {
      boardW,
      rowCount: rows.length,
      filled: rows.filter((r) => Math.abs(r.w - boardW) <= 2).length,
      heights: [...new Set(rows.map((r) => r.h))].slice(0, 12),
      perRowCards: [...new Set(rows.map((r) => r.n))],
      cardCount: document.querySelectorAll(".tl-card").length,
    };
  });

  console.log("\n── ① 行式密排 ──");
  console.log(`  容器宽 ${layout.boardW}px · 行数 ${layout.rowCount} · 卡片 ${layout.cardCount}`);
  console.log(`  铺满容器宽度的行：${layout.filled}/${layout.rowCount}`);
  console.log(`  行高取值（应有多个，且不等于固定值）：${layout.heights.join(",")}`);
  console.log(`  每行卡片数：${layout.perRowCards.join(",")}`);

  /* ── ② 左栏是否存在、sticky 是否生效、节点层级 ── */
  const rail = await p.evaluate(() => {
    const el = document.querySelector(".tl-rail");
    if (!el) return { exists: false };
    const cs = getComputedStyle(el);
    return {
      exists: true,
      position: cs.position,
      width: Math.round(el.getBoundingClientRect().width),
      years: [...el.querySelectorAll(".is-year")].map((n) => n.textContent.trim().replace(/\s+/g, " ")),
      months: el.querySelectorAll(".is-month").length,
      active: document.querySelector(".tl-rail-node.on")?.textContent?.trim().replace(/\s+/g, " "),
    };
  });
  console.log("\n── ② 左侧时间栏 ──");
  console.log(`  存在=${rail.exists} position=${rail.position} 宽=${rail.width}px`);
  console.log(`  年节点(${rail.years?.length})：${rail.years?.join(" | ")}`);
  console.log(`  月节点数量：${rail.months}`);
  console.log(`  当前高亮：${rail.active}`);

  /* ── ③ 滚动跟随：滚到中段，左栏高亮应变化 ── */
  const before = await p.evaluate(() => document.querySelector(".tl-rail-node.on")?.textContent?.trim());
  await p.evaluate(() => window.scrollTo({ top: 2400, behavior: "instant" }));
  await p.waitForTimeout(500);
  const after = await p.evaluate(() => document.querySelector(".tl-rail-node.on")?.textContent?.trim());
  const railTop = await p.evaluate(() => Math.round(document.querySelector(".tl-rail").getBoundingClientRect().top));
  console.log("\n── ③ 滚动 ──");
  console.log(`  滚动前高亮：${before}`);
  console.log(`  滚动后高亮：${after}  ${before !== after ? "（已跟随 ✓）" : "（未变化 ✗）"}`);
  console.log(`  滚动后左栏 top=${railTop}px（sticky 应保持在 ~68px）`);

  /* ── ④ 点击左栏跳转 ── */
  await p.evaluate(() => window.scrollTo({ top: 0, behavior: "instant" }));
  await p.waitForTimeout(300);
  const monthNodes = await p.$$(".tl-rail-node.is-month");
  let jumped = null;
  if (monthNodes.length > 2) {
    await monthNodes[2].click();
    await p.waitForTimeout(900);
    jumped = await p.evaluate(() => {
      const on = document.querySelector(".tl-rail-node.on");
      return { text: on?.textContent?.trim(), scrollY: Math.round(window.scrollY) };
    });
  }
  console.log("\n── ④ 点击左栏跳转 ──");
  console.log(`  ${jumped ? `点第 3 个月 → 高亮=${jumped.text} scrollY=${jumped.scrollY}` : "月份节点不足，跳过"}`);

  await p.evaluate(() => window.scrollTo({ top: 0, behavior: "instant" }));
  await p.waitForTimeout(300);
  const out = path.join(__dirname, "..", "design", "preview", `timeline-${tag}.png`);
  await p.screenshot({ path: out, fullPage: true });
  console.log(`\n截图：${path.basename(out)}`);
  if (errs.length) console.log("页面错误：\n  " + [...new Set(errs)].slice(0, 6).join("\n  "));
  else console.log("页面错误：无");

  await b.close();
})();
