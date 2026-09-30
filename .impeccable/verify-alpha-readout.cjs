/** 探针：主题弹窗「组件透明度的实测读数」是否如实——含 α=0 与浅顶带预设两种情形 */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push("pageerror: " + e.message));
  page.on("console", (m) => { if (m.type() === "error") errors.push("console: " + m.text()); });
  await page.addInitScript(INIT);

  const readout = async () => {
    const el = await page.$(".pm-range:has-text('组件透明度') + .pm-hint");
    return el ? (await el.textContent()).replace(/\s+/g, " ").trim() : "(未找到读数)";
  };
  const openDialog = async () => {
    // Home 页头那个「🎨」图标按钮就是入口（title="主题 / 皮肤设置"）
    await page.click("button[title='主题 / 皮肤设置']");
    await page.waitForSelector(".pm-dialog", { timeout: 8000 });
    await page.click(".pm-tab:has-text('自定义效果')").catch(() => {});
    await page.waitForTimeout(300);
  };

  await page.goto("http://localhost:1420/home", { waitUntil: "networkidle", timeout: 20000 });
  await page.waitForTimeout(1000);
  await openDialog();
  console.log("滑块 min =", await page.$eval("input[aria-label='组件玻璃透明度（不透明度）']", (el) => el.min));
  console.log("[默认外观 α=0.64] ", await readout());

  await page.fill("input[aria-label='组件玻璃透明度（不透明度）']", "0");
  await page.dispatchEvent("input[aria-label='组件玻璃透明度（不透明度）']", "change");
  await page.waitForTimeout(400);
  console.log("[默认外观 α=0]    ", await readout());
  const warnAtZero = await page.$eval(".pm-range:has-text('组件透明度') + .pm-hint", (el) => el.classList.contains("pm-warn"));
  console.log("  警示样式 =", warnAtZero);

  // 切到浅顶带预设（粉绿晨光，网格第一张）后再看 α=0 的读数
  await page.click(".pm-tab:has-text('预定义效果')");
  await page.waitForTimeout(200);
  await page.click(".pm-presets .pm-preset:first-child");
  await page.waitForTimeout(400);
  await page.click(".pm-tab:has-text('自定义效果')");
  await page.waitForTimeout(200);
  console.log("[预设 粉绿晨光 α=0.55]", await readout());
  await page.fill("input[aria-label='组件玻璃透明度（不透明度）']", "0");
  await page.dispatchEvent("input[aria-label='组件玻璃透明度（不透明度）']", "change");
  await page.waitForTimeout(400);
  console.log("[预设 粉绿晨光 α=0]", await readout(), "警示 =", await page.$eval(".pm-range:has-text('组件透明度') + .pm-hint", (el) => el.classList.contains("pm-warn")));

  // 回到默认卡 → 应恢复启动封面
  await page.click(".pm-tab:has-text('预定义效果')");
  await page.click(".pm-preset-default");
  await page.waitForTimeout(500);
  const bg = await page.evaluate(() => {
    const img = document.querySelector(".app-bg-img");
    const scrim = document.querySelector(".app-bg-scrim");
    return {
      hasImage: !!img,
      url: img ? getComputedStyle(img).backgroundImage.slice(0, 60) : "(无)",
      opacity: img ? getComputedStyle(img).opacity : "-",
      hasScrim: !!scrim,
      defaultCardOn: !!document.querySelector(".pm-preset-default.on"),
    };
  });
  console.log("[点默认卡后]", JSON.stringify(bg));
  console.log("[点默认卡后的偏好]", await page.evaluate(() => localStorage.getItem("pm-theme")));
  console.log("默认卡 class =", await page.$eval(".pm-preset-default", (el) => el.className));
  await page.screenshot({ path: ".impeccable/review/theme-alpha0-preset.png" });
  console.log("控制台错误:", errors.length ? errors.slice(0, 4).join(" | ") : "0");
  await browser.close();
})();
