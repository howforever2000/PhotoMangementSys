/**
 * FEAT-086 表面光学验收：同一页面分别截磨砂 / 液态两态，供肉眼比对材质是否可辨。
 * 用法：先起 dev server（1420），再 node .impeccable/material-compare.cjs
 */
const { chromium } = require("playwright");
const path = require("path");
const fs = require("fs");
const { INIT } = require("./mock-init.cjs");

const OUT = path.join(__dirname, "review");
fs.mkdirSync(OUT, { recursive: true });

(async () => {
  const browser = await chromium.launch({
    headless: true,
    channel: "msedge",
    args: ["--proxy-server=<direct>", "--proxy-bypass-list=*"],
  });
  const errors = [];
  for (const mat of ["frosted", "liquid"]) {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const page = await ctx.newPage();
    page.on("pageerror", (e) => errors.push(`[${mat}] ${e.message}`));
    await page.addInitScript(INIT);
    // 注入偏好：液态材质 + 一套浅色预设（背景明亮，材质差异最易辨）
    await page.addInitScript((m) => {
      localStorage.setItem(
        "pm-theme",
        JSON.stringify({
          mode: "dark",
          compColor: "#f4f8f6",
          compAlpha: 0.55,
          material: m,
          bgStyle: "gradient",
          bgColor: "#0e211b",
          gradFrom: "#ceffd1",
          gradMid: "#638196",
          gradTo: "#3a2e67",
          gradAngle: 180,
          bgOpacity: 0.45,
        }),
      );
    }, mat);
    await page.goto("http://localhost:1420/home", { waitUntil: "load", timeout: 60000 });
    await page.waitForTimeout(1500);
    const modalCount = await page.locator(".pm-modal").count();
    const bodyCls = await page.evaluate(() => document.body.className);
    console.log(`[${mat}] pm-modal=${modalCount} bodyClass=${bodyCls}`);
    if (modalCount !== 0) throw new Error(`[${mat}] 弹窗不应打开（pm-modal=${modalCount}）`);
    if (!bodyCls.includes(`mat-${mat}`)) throw new Error(`[${mat}] body 材质类未生效: ${bodyCls}`);
    // 把证据画进截图本身（页内角标），避免读图缓存/文件混淆歧义
    await page.evaluate((label) => {
      const div = document.createElement("div");
      div.id = "__proof__";
      div.style.cssText =
        "position:fixed;left:8px;top:8px;z-index:99999;background:#000;color:#0f0;font:14px monospace;padding:6px 10px;border-radius:6px";
      div.textContent = `${label} | modal=${document.querySelectorAll(".pm-modal").length} | ${document.body.className}`;
      document.body.appendChild(div);
    }, mat);
    // hover 中间卡片，让液态追光处于激活态
    const card = page.locator(".module-card").first();
    await card.hover().catch(() => {});
    await page.waitForTimeout(500);
    await page.screenshot({ path: path.join(OUT, `material-${mat}.png`) });
    console.log("captured:", mat);
    await ctx.close();
  }
  await browser.close();
  console.log("PAGE_ERRORS:", errors.length ? errors.join("\n") : "(none)");
})();
