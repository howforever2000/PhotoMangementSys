/* 一次性探针：检查 ThemeDialog 自定义页签里 .pm-hex 的计算样式是否继承到 --pm-input-bg */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

(async () => {
  const b = await chromium.launch({
    headless: true,
    channel: "msedge",
    args: ["--proxy-server=<direct>", "--proxy-bypass-list=*"],
  });
  const c = await b.newContext({ viewport: { width: 1440, height: 900 } });
  const p = await c.newPage();
  await p.addInitScript(INIT);
  await p.goto("http://localhost:1420/home", { waitUntil: "load", timeout: 20000 });
  await p.waitForTimeout(800);
  await p.click('button[title="主题 / 皮肤设置"]');
  await p.waitForTimeout(300);
  await p.locator(".pm-preset").nth(2).click();
  await p.waitForTimeout(400);
  await p.click("#tab-custom");
  await p.waitForTimeout(300);
  const info = await p.evaluate(() => {
    const el = document.querySelector(".pm-hex");
    const cs = getComputedStyle(el);
    const dlg = el.closest(".pm-dialog");
    return {
      hexBg: cs.backgroundColor,
      hexColor: cs.color,
      dlgVar: getComputedStyle(dlg).getPropertyValue("--pm-input-bg").trim(),
      dlgBg: getComputedStyle(dlg).backgroundColor,
    };
  });
  console.log(JSON.stringify(info, null, 1));
  await b.close();
})();
