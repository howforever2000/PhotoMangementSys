/**
 * FEAT-124 验收探针：人物卡改成「照片裁切 + 文字嵌入」
 *
 * 跑法：npx vite --port 1519  →  node .visual/probe-person-cover.cjs
 *
 * 断言：
 *   ① 卡片封面是 4:5 的图片（.pc-img），不再是圆形头像
 *   ② 文字压在同一张照片上（.pc-cap 与 .pc-img 在同一 .pc-cover 内）
 *   ③ 未命名人物显示「未命名」，命名人物显示名字
 *   ④ 页面无报错；get_person_covers_bulk 恰好 1 次
 */
const { chromium } = require("playwright");
const { writeFileSync } = require("fs");

const PORT = process.env.VITE_PORT || 1519;
const BASE = `http://localhost:${PORT}/.visual/preview.html#/smart?tab=face`;

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });

  await page.goto(BASE, { waitUntil: "load" });
  await page.waitForSelector(".pc-cover", { timeout: 20000 }).catch(() => {});
  await page.waitForTimeout(800);

  const info = await page.evaluate(() => {
    const cards = [...document.querySelectorAll(".person-card")];
    const first = cards[0];
    const cover = first?.querySelector(".pc-cover");
    const img = first?.querySelector(".pc-img");
    const cap = first?.querySelector(".pc-cap");
    const cr = cover?.getBoundingClientRect();
    return {
      cards: cards.length,
      withImg: cards.filter((c) => c.querySelector(".pc-img")).length,
      ratio: cr ? +(cr.width / cr.height).toFixed(2) : 0,
      capInsideCover: !!(cover && cap && cover.contains(cap)),
      imgInsideCover: !!(cover && img && cover.contains(img)),
      firstName: first?.querySelector(".pc-name")?.textContent?.trim() ?? "",
      firstMeta: first?.querySelector(".pc-meta")?.textContent?.replace(/\s+/g, " ").trim() ?? "",
      unnamed: cards.filter((c) => c.querySelector(".pc-name")?.textContent?.includes("未命名"))
        .length,
      calls: { ...window.__PM_MOCK_CALLS__ },
    };
  });

  writeFileSync(".visual/out/person-cover.png", await page.screenshot(), "binary");
  const summary = { ...info };
  console.log("人物卡:", summary);

  const problems = [];
  if (info.cards === 0) problems.push("没有渲染出人物卡");
  if (!info.imgInsideCover) problems.push("封面图不在 .pc-cover 内");
  if (!info.capInsideCover) problems.push("文字没有压在封面上（不满足「文字嵌入」）");
  if (Math.abs(info.ratio - 0.8) > 0.06) problems.push(`封面宽高比 ${info.ratio}（期望 4:5 = 0.8）`);
  if ((info.calls.get_person_covers_bulk ?? 0) !== 1)
    problems.push(`get_person_covers_bulk 调了 ${info.calls.get_person_covers_bulk} 次（期望 1）`);
  if (!info.unnamed) problems.push("没有「未命名」文案（mock 里有未命名人物）");
  if (errors.length) problems.push(`页面报错: ${errors.slice(0, 3).join(" | ")}`);

  console.log("\n结果:", problems.length ? `❌ ${problems.length} 项不达标` : "✅ 全部达标");
  for (const p of problems) console.log("  -", p);
  await browser.close();
  process.exit(problems.length ? 1 : 0);
})();
