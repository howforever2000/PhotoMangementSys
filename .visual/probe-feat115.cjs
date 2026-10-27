// 效果图（FEAT-115）容器与文字色档实测：node .visual/probe-feat115.cjs
// 判据与 probe-containers.cjs 完全一致，只是对象从真实页面换成静态效果图。
const { chromium } = require("playwright");
const path = require("path");

const CHROME = "/home/hao/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome";
const DIR = path.resolve(__dirname, "../design/preview/proposal");

const PAGES = [
  ["feat115-albums", "相册管理"],
  ["feat115-smart-overview", "全局概览"],
  ["feat115-smart-cards", "分类·人物"],
  ["feat115-scan", "扫描入库"],
  ["feat115-timeline", "时间线"],
];

(async () => {
  const b = await chromium.launch({ executablePath: CHROME, args: ["--no-sandbox"] });
  console.log("效果图".padEnd(10), "容器", "文字色档", "强调色");
  for (const [name, label] of PAGES) {
    const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
    const file = "file:///" + path.join(DIR, `${name}.html`).replace(/\\/g, "/");
    await p.goto(file, { waitUntil: "load" });
    await p.evaluate(() => document.fonts.ready);
    await p.waitForTimeout(300);
    const r = await p.evaluate(() => {
      const parse = (c) => {
        const m = c.match(/rgba?\(([^)]+)\)/);
        if (!m) return null;
        const v = m[1].split(",").map((x) => parseFloat(x));
        return { r: v[0], g: v[1], b: v[2], a: v.length > 3 ? v[3] : 1 };
      };
      let containers = 0;
      const colors = new Set();
      const accent = new Set();
      /* 底色基准不能取 body 的 backgroundColor —— 渐变写在 background-image 上，
         computed backgroundColor 是 transparent，差值恒真。
         改取页面右下角（一定是纯背景）的像素。 */
      const bg = { r: 0, g: 0, b: 0, a: 0 };
      for (const el of document.querySelectorAll("body *")) {
        const cs = getComputedStyle(el);
        if (cs.display === "none" || cs.visibility === "hidden") continue;
        const rc = el.getBoundingClientRect();
        if (rc.width < 24 || rc.height < 12 || rc.top > 900) continue;
        const c = parse(cs.backgroundColor);
        /* 容器判据（区分"容器"与"结构线"）：
           · 有填充：alpha > 0.06（发丝线/内描边都是 inset，不算填充）
           · 有外投影：box-shadow 里含非 inset 的那一层
           · 不含：border（1px 发丝分隔线）、inset 内描边（封面边界）—— 这些是结构线 */
        /* 阈值 0.04：.board 的填充是 0.055（深色）/ 0.42（浅色），
           发丝线级别的内描边不产生 backgroundColor（它们在 box-shadow 里），
           所以这里不需要很高的阈值；取 0.04 只为排掉 color-mix 的舍入残渣。 */
        const hasFill = c && c.a > 0.04;
        /* ⚠️ 不能直接 split(",")：box-shadow 里 rgba(0,0,0,.5) 自带逗号，
           会把一层 inset 投影切成多段，"inset" 只落在第一段 → 含 inset 的
           多层投影被误判成"有外投影"（实测虚高 5 倍）。
           按括号深度切顶层逗号才对。 */
        let depth = 0, parts = [], cur = "";
        for (const ch of cs.boxShadow || "") {
          if (ch === "(") depth++;
          if (ch === ")") depth--;
          if (ch === "," && depth === 0) { parts.push(cur); cur = ""; continue; }
          cur += ch;
        }
        if (cur.trim()) parts.push(cur);
        /* ⚠️ box-shadow: none 会被切成一层字面量 "none" —— 它既非空也不含 inset，
           直接 some(!includes("inset")) 会把**没有投影**的元素全算成"有外投影"
           （实测虚高 5 倍）。必须先剔掉 none / 空串。 */
        const outer = parts.some((s) => s.trim() && s.trim() !== "none" && !s.includes("inset"));
        const isPhoto = ["IMG", "FIGURE", "PICTURE", "SVG"].includes(el.tagName);
        if ((hasFill || outer) && !isPhoto && rc.width * rc.height > 4000) containers++;
        const txt = [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
        if (txt && cs.color) {
          const cc = parse(cs.color);
          if (cc) {
            colors.add([cc.r, cc.g, cc.b].map((x) => Math.round(x / 16)).join(","));
            const mx = Math.max(cc.r, cc.g, cc.b), mn = Math.min(cc.r, cc.g, cc.b);
            if (mx - mn > 40 && cc.r > cc.g && cc.g >= cc.b) accent.add(cs.color);
          }
        }
      }
      return { containers, colors: colors.size, accent: accent.size };
    });
    console.log(label.padEnd(10), String(r.containers).padStart(4), String(r.colors).padStart(8), String(r.accent).padStart(6));
    await p.close();
  }
  await b.close();
})();
