/**
 * FEAT-086 主题弹窗冒烟：打开主题弹窗 → 预定义页签 → 套用第 3 套预设 → 自定义页签截图。
 * 纯开发工具。用法：先起 dev server（npx vite，1420），再 node .impeccable/theme-dialog-check.cjs
 */
const { chromium } = require("playwright");
const path = require("path");
const fs = require("fs");
const { INIT } = require("./mock-init.cjs");

const OUT = path.join(__dirname, "review");
fs.mkdirSync(OUT, { recursive: true });

(async () => {
  // 本机代理会拦 localhost：强制直连
  const browser = await chromium.launch({
    headless: true,
    channel: "msedge",
    args: ["--proxy-server=<direct>", "--proxy-bypass-list=*"],
  });
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await ctx.newPage();
  const errors = [];
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  await page.addInitScript(INIT);
  // vite dev 的 HMR websocket 会让 networkidle 永不触发，用 load + 固定等待
  await page.goto("http://localhost:1420/home", { waitUntil: "load", timeout: 20000 });
  await page.waitForTimeout(800);

  // 打开主题弹窗（🎨）
  await page.click('button[title="主题 / 皮肤设置"]');
  await page.waitForTimeout(400);
  await page.screenshot({ path: path.join(OUT, "theme-preset-tab.png") });

  // 套用第 3 套预设（薄荷靛蓝）并截图整页背景
  await page.locator(".pm-preset").nth(2).click();
  await page.waitForTimeout(500);
  await page.screenshot({ path: path.join(OUT, "theme-preset-applied.png") });

  // 切到自定义页签
  await page.click('#tab-custom');
  await page.waitForTimeout(400);
  await page.screenshot({ path: path.join(OUT, "theme-custom-tab.png") });

  // 材质切到液态，关闭弹窗看全局
  await page.getByRole("button", { name: "液态玻璃" }).click();
  await page.waitForTimeout(300);
  await page.keyboard.press("Escape");
  await page.waitForTimeout(400);
  await page.screenshot({ path: path.join(OUT, "theme-liquid-global.png") });

  const presetCount = await page.locator(".pm-preset").count().catch(() => -1);
  await browser.close();
  console.log("PRESET_COUNT_WHEN_CLOSED:", presetCount);
  console.log("CONSOLE_ERRORS:", errors.length ? "\n" + [...new Set(errors)].slice(0, 10).join("\n") : "(none)");
})();
