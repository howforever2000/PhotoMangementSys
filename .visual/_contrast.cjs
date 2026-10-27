/* 效果图文字对比度实测（WCAG）：node .visual/_contrast.cjs */
const { chromium } = require("playwright");
const path = require("path");
const CHROME = "/home/hao/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome";
const DIR = path.resolve(__dirname, "../design/preview/proposal");
const PAGES = ["feat115-albums","feat115-smart-overview","feat115-smart-cards","feat115-scan","feat115-timeline","feat115-memories-search-detail"];

function lum(r,g,b){const f=c=>{c/=255;return c<=0.03928?c/12.92:Math.pow((c+0.055)/1.055,2.4)};return 0.2126*f(r)+0.7152*f(g)+0.0722*f(b)}
function ratio(a,b){const l1=lum(...a),l2=lum(...b);return (Math.max(l1,l2)+0.05)/(Math.min(l1,l2)+0.05)}

(async () => {
  const b = await chromium.launch({ executablePath: CHROME, args: ["--no-sandbox"] });
  console.log("页面".padEnd(26), "最小对比度", "不达标条数(阈值 4.5)");
  for (const theme of ["pine-soft","celadon-soft"]) {
    console.log(`\n── ${theme} ──`);
    for (const name of PAGES) {
      const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
      await p.goto("file:///" + path.join(DIR, name + ".html"), { waitUntil: "load" });
      await p.evaluate(t => { document.body.dataset.theme = t; }, theme);
      await p.waitForTimeout(300);
      const r = await p.evaluate(() => {
        const parse = c => { const m=c.match(/rgba?\(([^)]+)\)/); if(!m) return null; const v=m[1].split(",").map(Number); return {r:v[0],g:v[1],b:v[2],a:v.length>3?v[3]:1}; };
        const rootBg = midBg ? {r:midBg[0],g:midBg[1],b:midBg[2],a:1} : {r:0,g:0,b:0,a:1};
        const items = [];
        // 渐变背景写在 body 的 background-image 上，computed backgroundColor 是 transparent。
        // 没有可用的纯色基准时，用页面四角采样（用元素中心点到视口角的连线取最远的角）
        // 太复杂 —— 直接给一个保守基准：取 --bg-2（渐变中段，视觉中心区域）。
        const bgVar = getComputedStyle(document.body).getPropertyValue("--bg-2").trim();
        const hex2rgb = h => { const n = parseInt(h.slice(1), 16); return [(n>>16)&255, (n>>8)&255, n&255]; };
        const midBg = bgVar.startsWith("#") ? hex2rgb(bgVar) : null;
        for (const el of document.querySelectorAll("body *")) {
          const cs = getComputedStyle(el);
          if (cs.display === "none" || cs.visibility === "hidden") continue;
          const rc = el.getBoundingClientRect();
          if (rc.width < 4 || rc.height < 4 || rc.top > 900 || rc.bottom < 0) continue;
          const hasText = [...el.childNodes].some(n => n.nodeType === 3 && n.textContent.trim());
          if (!hasText) continue;
          const fg = parse(cs.color);
          if (!fg) continue;
          // 逐层向上找第一个不透明背景
          let bg = rootBg, node = el;
          while (node && node !== document.body) {
            const c = parse(getComputedStyle(node).backgroundColor);
            if (c && c.a > 0.55) { bg = c; break; }
            node = node.parentElement;
          }
          // 前景 alpha 合成到底色上
          const comp = [0,1,2].map(i => Math.round(fg[i]*fg.a + bg[i]*(1-fg.a)));
          items.push({ fg: comp, bg: [bg.r,bg.g,bg.b], size: parseFloat(cs.fontSize), text: el.textContent.trim().slice(0,14) });
        }
        return items;
      });
      let min = 99, bad = 0;
      const bads = [];
      for (const it of r) {
        const cr = ratio(it.fg, it.bg);
        if (cr < min) min = cr;
        if (cr < 4.5) { bad++; if (bads.length < 3) bads.push(`${it.size}px "${it.text}" ${cr.toFixed(2)}`); }
      }
      console.log(name.replace("feat115-","").padEnd(26), min.toFixed(2).padStart(6), String(bad).padStart(6), bads.length ? " ← " + bads.join(" | ") : "");
      await p.close();
    }
  }
  await b.close();
})();
