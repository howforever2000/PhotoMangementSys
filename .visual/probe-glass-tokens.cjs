// 验证 FEAT-108 令牌冲突修复：顶栏玻璃是否真的开始模糊、边框是否回到 1px
const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 1000 } });
  p.on("pageerror", (e) => console.log("pageerror", String(e).split("\n")[0]));

  await p.goto(`http://localhost:${PORT}/.visual/themed-probe.html#dark/AlbumList`, { waitUntil: "load" });
  await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 20000 }).catch(() => {});
  await p.waitForTimeout(1000);

  const r = await p.evaluate(() => {
    const pick = (sel) => {
      const el = document.querySelector(sel);
      if (!el) return { sel, missing: true };
      const cs = getComputedStyle(el);
      return {
        sel,
        backdropFilter: cs.backdropFilter,
        bg: cs.backgroundColor,
        border: `${cs.borderTopWidth} ${cs.borderTopStyle} ${cs.borderTopColor}`,
      };
    };
    const root = getComputedStyle(document.documentElement);
    return {
      header: pick(".mh-inner"),
      tok: {
        glassBlur: root.getPropertyValue("--glass-blur").trim(),
        floatBlur: root.getPropertyValue("--float-blur").trim(),
        floatBorder: root.getPropertyValue("--float-border").trim(),
        glassBorder: root.getPropertyValue("--glass-border").trim(),
      },
    };
  });
  console.log(JSON.stringify(r, null, 1));

  const ok =
    r.header.backdropFilter !== "none" &&
    /* FEAT-108：顶栏现在应该是「url("#liquid-lens") blur(8px) saturate(...)」
       ⚠️ Chromium 的 computed 值会给 url 加**引号**，所以正则要容差。 */
    /url\(["']?#liquid-lens["']?\)/.test(r.header.backdropFilter) &&
    /blur\([1-9][0-9]?px\)/.test(r.header.backdropFilter) &&
    r.header.border.startsWith("1px");
  console.log(
    ok
      ? "\nPASS 顶栏玻璃：折射滤镜已接入 + 模糊生效 + 1px 边框"
      : "\nFAIL 仍有问题",
  );
  await b.close();
  process.exit(ok ? 0 : 1);
})();
