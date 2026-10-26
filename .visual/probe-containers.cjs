// 容器与文字色档数实测：node .visual/probe-containers.cjs [页面...]
// 判据（沿用 design/preview/proposal/README.md §五 验证口径）：
//   ① 一屏内「有底/有边/有影」的块级元素个数（视觉容器的直接代理）
//   ② 一屏内去重后的文字 color 档数
//   ③ 强调色（--accent / 金）出现次数
const { chromium } = require("playwright");

const ROUTES = {
  Home: "/home", AlbumList: "/albums", SmartAlbum: "/smart", ScanHub: "/scan",
  Memories: "/memories", Timeline: "/timeline", SmartSearch: "/search",
  Workshop: "/workshop", AlbumDetail: "/album/1",
};
const PORT = process.env.VITE_PORT || "1519";

(async () => {
  const names = process.argv.slice(2).length ? process.argv.slice(2) : Object.keys(ROUTES);
  const b = await chromium.launch({ executablePath: "/home/hao/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome", args: ["--no-sandbox"] });
  console.log("页面".padEnd(12), "容器", "文字色档", "强调色", "块级元素");
  for (const n of names) {
    const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
    await p.goto(`http://localhost:${PORT}${ROUTES[n]}?mock=true`, { waitUntil: "load" });
    await p.waitForTimeout(1200);
    const r = await p.evaluate(() => {
      const parse = (c) => {
        const m = c.match(/rgba?\(([^)]+)\)/);
        if (!m) return null;
        const v = m[1].split(",").map((x) => parseFloat(x));
        return { r: v[0], g: v[1], b: v[2], a: v.length > 3 ? v[3] : 1 };
      };
      const accent = new Set();
      let containers = 0, blocks = 0;
      const colors = new Set();
      const rootBg = parse(getComputedStyle(document.body).backgroundColor) || { r: 0, g: 0, b: 0 };
      for (const el of document.querySelectorAll("body *")) {
        const cs = getComputedStyle(el);
        if (cs.display === "none" || cs.visibility === "hidden") continue;
        const rc = el.getBoundingClientRect();
        if (rc.width < 24 || rc.height < 12) continue;
        if (rc.top > 900) continue; // 只算首屏
        blocks++;
        const bg = parse(cs.backgroundColor);
        const hasFill = bg && bg.a > 0.06 && Math.abs(bg.r - rootBg.r) + Math.abs(bg.g - rootBg.g) + Math.abs(bg.b - rootBg.b) > 12;
        const hasBorder = parseFloat(cs.borderTopWidth) > 0 && parse(cs.borderTopColor)?.a > 0.12;
        const hasShadow = cs.boxShadow && cs.boxShadow !== "none";
        const isPhoto = el.tagName === "IMG" || el.tagName === "FIGURE" || el.tagName === "PICTURE";
        if ((hasFill || hasBorder || hasShadow) && !isPhoto && rc.width * rc.height > 4000) containers++;
        // 文字色
        const txt = [...el.childNodes].some((c) => c.nodeType === 3 && c.textContent.trim());
        if (txt && cs.color) {
          const c = parse(cs.color);
          if (c) {
            // 量化到 4bit，避免同一色因 color-mix 抖动算成多档
            colors.add([c.r, c.g, c.b].map((x) => Math.round(x / 16)).join(","));
            const hex = "#" + [c.r, c.g, c.b].map((x) => Math.round(x).toString(16).padStart(2, "0")).join("");
            // 金/暖强调（hue 30~55 且饱和度高）
            const mx = Math.max(c.r, c.g, c.b), mn = Math.min(c.r, c.g, c.b);
            if (mx - mn > 40 && c.r > c.g && c.g >= c.b) accent.add(hex);
          }
        }
      }
      return { containers, colors: colors.size, accent: accent.size, blocks };
    });
    console.log(n.padEnd(12), String(r.containers).padStart(4), String(r.colors).padStart(8), String(r.accent).padStart(6), String(r.blocks).padStart(8));
    await p.close();
  }
  await b.close();
})();
