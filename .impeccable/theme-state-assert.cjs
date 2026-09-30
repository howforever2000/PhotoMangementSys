/* FEAT-086 状态断言探针：逐步验证预设应用/页签切换/材质切换的可观测状态 */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

const assert = (cond, msg) => {
  if (!cond) throw new Error("ASSERT FAIL: " + msg);
  console.log("PASS:", msg);
};

(async () => {
  const b = await chromium.launch({
    headless: true,
    channel: "msedge",
    args: ["--proxy-server=<direct>", "--proxy-bypass-list=*"],
  });
  const c = await b.newContext({ viewport: { width: 1440, height: 900 } });
  const p = await c.newPage();
  const errors = [];
  p.on("pageerror", (e) => errors.push(e.message));
  await p.addInitScript(INIT);
  await p.goto("http://localhost:1420/home", { waitUntil: "load", timeout: 60000 });
  await p.waitForTimeout(800);

  // 1. 打开弹窗 → 预设页签默认激活，5 张卡片
  await p.click('button[title="主题 / 皮肤设置"]');
  await p.waitForTimeout(300);
  assert((await p.locator(".pm-preset").count()) === 5, "预设卡片 5 张");
  assert(
    (await p.getAttribute("#tab-preset", "aria-selected")) === "true",
    "默认激活「预定义效果」页签",
  );
  assert((await p.locator(".pm-preset.on").count()) === 0, "初始无选中预设");

  // 2. 套用第 3 套（薄荷靛蓝）
  await p.locator(".pm-preset").nth(2).click();
  await p.waitForTimeout(400);
  assert((await p.locator(".pm-preset.on").count()) === 1, "套用后恰好 1 张卡片选中");
  const badge = await p.locator(".pm-preset.on .pm-preset-badge").textContent();
  assert(/磨砂/.test(badge || ""), "选中卡片徽章显示磨砂材质: " + badge);
  const bodyBg = await p.evaluate(() => getComputedStyle(document.querySelector(".app-bg-base")).backgroundImage);
  assert(bodyBg.includes("linear-gradient"), "页面底层已切为渐变: " + bodyBg.slice(0, 60));
  const matClass = await p.evaluate(() => document.body.className);
  assert(matClass.includes("mat-frosted"), "body 挂 mat-frosted: " + matClass);

  // 2.1 角度滑块：默认可用（已套预设=渐变），拖到 90° 后背景实时变化且不脱离选中态
  const angleDisabled = await p.evaluate(
    () => document.querySelector('#panel-preset .pm-range input[type="range"]').disabled,
  );
  assert(angleDisabled === false, "套用预设后角度滑块可用");
  await p.evaluate(() => {
    const el = document.querySelector('#panel-preset .pm-range input[type="range"]');
    el.value = "90";
    // v-model 监听 input；change 仅触发 persist
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await p.waitForTimeout(300);
  const bodyBg90 = await p.evaluate(() => getComputedStyle(document.querySelector(".app-bg-base")).backgroundImage);
  assert(bodyBg90.includes("90deg"), "角度改 90° 背景实时生效: " + bodyBg90.slice(0, 50));
  assert((await p.locator(".pm-preset.on").count()) === 1, "调角度不脱离预设选中态");
  const swatchBg = await p.evaluate(
    () => getComputedStyle(document.querySelector(".pm-preset.on .pm-preset-swatch")).backgroundImage,
  );
  assert(swatchBg.includes("90deg"), "预设卡片色卡同步当前角度: " + swatchBg.slice(0, 50));

  // 3. 切自定义页签：值与预设联动一致
  await p.click("#tab-custom");
  await p.waitForTimeout(300);
  const hex = await p.inputValue(".pm-hex");
  assert(hex === "#f4f8f6", "自定义页签组件色调 = 预设值 #f4f8f6, got " + hex);
  const alpha = await p.evaluate(
    () => document.querySelector('#panel-custom .pm-range input[type="range"]').value,
  );
  assert(Number(alpha) > 0.5, "组件透明度滑块带出预设 55%: " + alpha);
  const hexBg = await p.evaluate(() => getComputedStyle(document.querySelector(".pm-hex")).backgroundColor);
  assert(!hexBg.includes("18, 20, 28"), "hex 输入框未被 theme-dark 兜底刷黑: " + hexBg);

  // 4. 材质切液态
  await p.getByRole("button", { name: "液态玻璃" }).click();
  await p.waitForTimeout(300);
  const mat2 = await p.evaluate(() => document.body.className);
  assert(mat2.includes("mat-liquid") && !mat2.includes("mat-frosted"), "body 切 mat-liquid: " + mat2);

  // 5. Esc 关闭
  await p.keyboard.press("Escape");
  await p.waitForTimeout(300);
  assert((await p.locator(".pm-modal").count()) === 0, "Esc 关闭弹窗");

  // 6. 重开：状态持久化（仍为液态 + 预设选中）
  await p.click('button[title="主题 / 皮肤设置"]');
  await p.waitForTimeout(300);
  assert((await p.locator(".pm-preset.on").count()) === 1, "重开后预设选中态保持");
  await p.click("#tab-custom");
  await p.waitForTimeout(200);
  const liquidOn = await p.evaluate(() => {
    const btns = [...document.querySelectorAll(".pm-seg button")];
    const liquid = btns.find((x) => x.textContent.includes("液态"));
    return liquid.classList.contains("on");
  });
  assert(liquidOn, "重开后液态材质选中态保持");

  // 7. 恢复默认（bgStyle=color）→ 预定义页签角度滑块应禁用
  await p.getByRole("button", { name: "恢复默认" }).click();
  await p.waitForTimeout(300);
  const hexAfterReset = await p.inputValue(".pm-hex");
  assert(hexAfterReset === "#16443a", "恢复默认后组件色调回 #16443a: " + hexAfterReset);
  await p.click("#tab-preset");
  await p.waitForTimeout(200);
  const angleDisabled2 = await p.evaluate(
    () => document.querySelector('#panel-preset .pm-range input[type="range"]').disabled,
  );
  assert(angleDisabled2 === true, "恢复默认（纯色背景）后角度滑块禁用");

  await b.close();
  assert(errors.length === 0, "全程无 pageerror: " + errors.join("; "));
  console.log("ALL PASS");
})().catch((e) => {
  console.error(e.message);
  process.exit(1);
});
