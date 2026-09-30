/** 列出当前路由上所有带渐变背景的元素（定位「彩虹色」到底是谁） */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

(async () => {
  const route = process.argv[2] || "/albums";
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await ctx.newPage();
  await page.addInitScript(INIT);
  await page.goto(`http://localhost:1420${route}`, { waitUntil: "networkidle", timeout: 20000 }).catch(() => {});
  await page.waitForTimeout(1200);
  const list = await page.evaluate(() => {
    const out = [];
    for (const el of document.querySelectorAll("body *")) {
      const img = getComputedStyle(el).backgroundImage || "";
      if (!img.includes("gradient")) continue;
      const r = el.getBoundingClientRect();
      if (r.width < 8 || r.height < 8) continue;
      out.push({
        cls: (el.className || "").toString().slice(0, 50) || el.tagName,
        w: Math.round(r.width), h: Math.round(r.height),
        top: Math.round(r.top),
        grad: img.replace(/\s+/g, " ").slice(0, 120),
      });
    }
    return out;
  });
  console.log(route, "→", list.length, "个渐变元素");
  for (const x of list) console.log(`  [${x.w}x${x.h} top=${x.top}] .${x.cls}\n      ${x.grad}`);
  await browser.close();
})();
