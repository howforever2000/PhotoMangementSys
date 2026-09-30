/**
 * 饱和度管线量化探针：同一页面在三档饱和度下，读取关键计算色值，
 * 用于证明「背景/组件色调/品牌色/语义色/链接色」都被同一管线统一带动。
 * 用法：node .impeccable\probe-sat.cjs
 */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

const SATS = [0.4, 1, 1.6, 2];

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const rows = [];
  for (const sat of SATS) {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const page = await ctx.newPage();
    await page.addInitScript(INIT);
    await page.addInitScript((s) => {
      const p = JSON.parse(localStorage.getItem("pm-theme") || "{}");
      localStorage.setItem("pm-theme", JSON.stringify({ ...p, saturation: s }));
    }, sat);
    await page.goto("http://localhost:1420/home", { waitUntil: "domcontentloaded", timeout: 20000 });
    await page.waitForTimeout(1800);
    const r = await page.evaluate(() => {
      const cs = getComputedStyle(document.body);
      const card = document.querySelector(".module-card");
      const arrow = document.querySelector(".module-arrow");
      const bgLayer = document.querySelector(".app-bg-base");
      return {
        saturation: cs.getPropertyValue("--saturation-probe").trim(),
        primary: cs.getPropertyValue("--color-primary").trim(),
        link: cs.getPropertyValue("--color-link").trim(),
        linkOnBg: cs.getPropertyValue("--color-link-on-bg").trim(),
        ok: cs.getPropertyValue("--color-ok").trim(),
        okText: cs.getPropertyValue("--color-ok-text").trim(),
        dangerText: cs.getPropertyValue("--color-danger-text").trim(),
        compTone: cs.getPropertyValue("--color-surface").trim(),
        glassBg: cs.getPropertyValue("--glass-bg").trim(),
        pageBg: bgLayer ? getComputedStyle(bgLayer).backgroundColor : "-",
        pageBgImage: bgLayer ? getComputedStyle(bgLayer).backgroundImage.slice(0, 40) : "-",
        arrowColor: arrow ? getComputedStyle(arrow).color : "-",
        cardBg: card ? getComputedStyle(card).backgroundColor : "-",
      };
    });
    rows.push([sat, r]);
    await ctx.close();
  }
  await browser.close();

  const keys = [
    "primary", "link", "linkOnBg", "ok", "okText", "dangerText",
    "compTone", "glassBg", "pageBg", "arrowColor", "cardBg",
  ];
  for (const k of keys) {
    console.log(`\n${k}:`);
    for (const [sat, r] of rows) console.log(`  ×${sat}  ${r[k]}`);
  }
})();
