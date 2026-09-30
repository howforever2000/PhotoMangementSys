/* 四通道材质验证：读取伪元素/令牌的计算样式，证明磨砂 vs 液态在
   填充/高光/纹理/边缘/投影五个通道上都不同（功能性证据，不依赖读图）。 */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

const PREF = (material) =>
  JSON.stringify({
    mode: "dark",
    compColor: "#16443a",
    compAlpha: 0.42,
    material,
    bgStyle: "gradient",
    bgColor: "#0e211b",
    gradFrom: "#ceffd1",
    gradMid: "#638196",
    gradTo: "#3a2e67",
    gradAngle: 180,
    bgOpacity: 0.45,
  });

const assert = (cond, msg) => {
  console.log((cond ? "PASS" : "FAIL") + ": " + msg);
  if (!cond) process.exitCode = 1;
};

(async () => {
  const browser = await chromium.launch({
    headless: true,
    channel: "msedge",
    args: ["--proxy-server=<direct>", "--proxy-bypass-list=*"],
  });

  const probe = async (material) => {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const page = await ctx.newPage();
    await page.addInitScript(INIT);
    await page.addInitScript((m) => localStorage.setItem("pm-theme", PREF(m)), material);
    await page.goto("http://localhost:1420/home", { waitUntil: "load", timeout: 60000 });
    await page.waitForSelector(".module-card", { timeout: 20000 });
    await page.waitForTimeout(800);
    const r = await page.evaluate(() => {
      const card = document.querySelector(".module-card");
      const cs = getComputedStyle(card);
      const before = getComputedStyle(card, "::before");
      const after = getComputedStyle(card, "::after");
      return {
        blur: cs.backdropFilter,
        shadow: cs.boxShadow.slice(0, 200),
        fill: cs.backgroundColor,
        beforeBg: (before.backgroundImage || "").slice(0, 120),
        beforeBlend: before.mixBlendMode,
        beforeFilter: before.filter,
        beforeOpacity: before.opacity,
        afterBg: (after.backgroundImage || "").slice(0, 80),
        afterTransition: after.transitionDuration,
        pos: cs.position,
      };
    });
    await ctx.close();
    return r;
  };

  const fro = await probe("frosted");
  const liq = await probe("liquid");

  console.log("--- frosted ---"); console.log(fro);
  console.log("--- liquid ---"); console.log(liq);

  // 通道 1：模糊/饱和（边缘光学）
  assert(fro.blur.includes("24px") && !fro.blur.includes("10px"), `磨砂 blur 24px: ${fro.blur}`);
  assert(liq.blur.includes("10px"), `液态 blur 10px: ${liq.blur}`);

  // 通道 2：投影（磨砂柔和 vs 液态更深）
  assert(fro.shadow !== liq.shadow, "两材质 box-shadow 不同（令牌分材质覆写）");
  assert(liq.shadow.includes("0.38") && fro.shadow.includes("0.2"), "液态投影更深 (0.38 vs 0.20)");

  // 通道 3：填充（液态掺白提亮）
  assert(liq.fill !== fro.fill, `填充不同: fro=${fro.fill} liq=${liq.fill}`);
  const parse = (s) => s.match(/[\d.]+/g).slice(0, 3).map(Number);
  const [fr, fg, fb] = parse(fro.fill), [lr, lg, lb] = parse(liq.fill);
  assert(lr > fr && lg > fg && lb > fb, `液态填充更亮 (${lr},${lg},${lb}) > (${fr},${fg},${fb})`);

  // 通道 4：纹理（磨砂噪点 soft-light vs 液态光池 blur）
  assert(fro.beforeBg.includes("url(") && fro.beforeBlend === "soft-light", `磨砂 ::before = 噪点+soft-light: ${fro.beforeBlend}`);
  assert(liq.beforeBg.includes("radial-gradient") && liq.beforeFilter.includes("4px"), "液态 ::before = 光池+blur(4px)");
  assert(fro.beforeBg !== liq.beforeBg, "两材质 ::before 图层不同");

  // 通道 5：追光层（仅液态存在）
  assert(after(liq.afterBg) && after(fro.afterBg) === false, `::after 追光层仅液态: liq=${liq.afterBg.slice(0, 40)}`);
  function after(bg) { return bg.includes("radial-gradient"); }
  assert(liq.afterTransition.startsWith("0.25s"), `追光层过渡 0.25s: ${liq.afterTransition}`);

  // 结构：伪元素定位前提
  assert(fro.pos === "relative" && liq.pos === "relative", "position: relative 生效");

  await browser.close();
})();
