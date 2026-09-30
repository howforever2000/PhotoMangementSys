/**
 * 修复验证：浅色预设下 AlbumList（普通 + 手动排序模式）与 TestScan 的文字可读性。
 */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

const LIGHT = {
  bgStyle: "gradient",
  gradFrom: "#ffd7d7",
  gradMid: "#7e9963",
  gradTo: "#195e53",
  compColor: "#f2f9f4",
  compAlpha: 0.55,
  material: "frosted",
};

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const ctx = await browser.newContext({ viewport: { width: 1760, height: 920 } });
  const page = await ctx.newPage();
  await page.addInitScript(INIT);
  await page.addInitScript((p) => {
    const prefs = JSON.parse(localStorage.getItem("pm-theme") || "{}");
    localStorage.setItem("pm-theme", JSON.stringify({ ...prefs, ...p }));
  }, LIGHT);

  // 1) /albums 普通模式（面包屑/工具条）
  await page.goto("http://localhost:1420/albums", { waitUntil: "domcontentloaded", timeout: 20000 });
  await page.waitForTimeout(2200);
  await page.screenshot({ path: ".impeccable/review/fix-albums.png" });
  console.log("captured: albums");

  // 2) 切到手动排序模式（页面上的排序方式下拉）
  const sortSel = await page.$("select");
  if (sortSel) {
    const opts = await sortSel.$$eval("option", (os) => os.map((o) => ({ v: o.value, t: o.textContent })));
    console.log("sort options:", JSON.stringify(opts));
    const manual = opts.find((o) => /手动|manual/i.test(o.t) || /manual/i.test(o.v));
    if (manual) {
      await page.selectOption("select", manual.v);
      await page.waitForTimeout(1500);
      await page.screenshot({ path: ".impeccable/review/fix-manual-sort.png" });
      console.log("captured: manual-sort");
    } else {
      console.log("manual option not found");
    }
  } else {
    console.log("no select found");
  }

  // 3) /scan/test
  await page.goto("http://localhost:1420/scan/test", { waitUntil: "domcontentloaded", timeout: 20000 });
  await page.waitForTimeout(2000);
  await page.screenshot({ path: ".impeccable/review/fix-testscan.png" });
  console.log("captured: testscan");

  await browser.close();
})();
