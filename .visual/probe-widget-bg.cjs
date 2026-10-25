// 查 widget 板实际合成的背景色/alpha：node .visual/probe-widget-bg.cjs [variant]
const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
const variant = process.argv[2] || "wall";

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 1 });
  await p.goto(`http://localhost:${PORT}/.visual/overview-probe.html#${variant}`, { waitUntil: "load" });
  await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 25000 }).catch(() => {});
  await p.waitForTimeout(500);
  const info = await p.evaluate(() => {
    const el = document.querySelector(".ui-widget");
    const cs = getComputedStyle(el);
    const root = getComputedStyle(document.documentElement);
    const bodyCs = getComputedStyle(document.body);
    const body = bodyCs;
    return {
      widgetBg: cs.backgroundColor,
      widgetBgImage: cs.backgroundImage.slice(0, 200),
      widgetBackdrop: cs.backdropFilter,
      widgetBorder: cs.borderTopColor,
      widgetShadow: cs.boxShadow,
      varColorSurface: body.getPropertyValue("--color-surface").trim() || root.getPropertyValue("--color-surface").trim(),
      varColorSurfaceRoot: root.getPropertyValue("--color-surface").trim(),
      varWLine: root.getPropertyValue("--w-line").trim(),
      varWInner: root.getPropertyValue("--w-inner-line").trim(),
      varWOuter: root.getPropertyValue("--w-outer-line").trim(),
      bodyBg: body.backgroundColor,
      bodyBgImage: body.backgroundImage.slice(0, 120),
      themeDark: document.body.classList.contains("theme-dark"),
    };
  });
  console.log(JSON.stringify(info, null, 2));
  await b.close();
})();
