/**
 * 临时脚本：打开主题弹窗 → 自定义效果页签，验证三颗材质按钮布局。
 */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
  await page.addInitScript(INIT);
  await page.goto("http://localhost:1420/home", { waitUntil: "domcontentloaded", timeout: 15000 });
  await page.waitForTimeout(2000);
  const btn = await page.$('button[title*="主题"], .theme-btn, button:has-text("🎨")');
  if (btn) {
    await btn.click();
    await page.waitForTimeout(800);
  }
  const tab = await page.$('button:has-text("自定义效果"), [id*="tab-custom"]');
  if (tab) {
    await tab.click();
    await page.waitForTimeout(500);
  }
  await page.screenshot({ path: ".impeccable/review/theme-glazed-seg.png" });
  console.log("done");
  await browser.close();
})();
