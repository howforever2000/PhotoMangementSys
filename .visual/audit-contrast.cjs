// 多页 × 深浅的对比度审计 + 截图
const { chromium } = require("playwright");

const AUDIT = () => {
  /**
   * 解析 getComputedStyle 里的颜色字符串。
   *
   * ⚠️ Chromium 会把 `color-mix()` / `oklch()` 等现代语法的**计算值**
   * 输出成 `color(srgb 0.95 0.94 0.96 / 0.05)` —— 分量是 0~1 而不是 0~255。
   * 旧解析直接当成 0~255 读，于是所有用了 color-mix 的元素都被算成“深灰”，
   * 报出一大片假的低对比（实测 ScanHub 三个假阳性全部是这个原因）。
   */
  const parse = (s) => {
    if (!s) return null;
    const str = String(s).trim();
    const srgb = str.match(/^color\(srgb\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+))?\)/i);
    if (srgb) {
      return {
        r: +srgb[1] * 255,
        g: +srgb[2] * 255,
        b: +srgb[3] * 255,
        a: srgb[4] === undefined ? 1 : +srgb[4],
      };
    }
    // 其他无法可靠解析的色彩空间（oklab / lab / lch）：跳过，不猜
    if (/^(oklab|lab|lch|oklch|color\()/i.test(str)) return null;
    const m = str.match(/[\d.]+/g);
    if (!m || m.length < 3) return null;
    return { r: +m[0], g: +m[1], b: +m[2], a: m[3] === undefined ? 1 : +m[3] };
  };
  const lum = ([r, g, b]) => {
    const f = (v) => {
      v /= 255;
      return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
    };
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
  };
  const ratio = (a, c) => {
    const [x, y] = [lum(a), lum(c)].sort((m, n) => n - m);
    return (x + 0.05) / (y + 0.05);
  };
  const dark = document.body.classList.contains("theme-dark");

  /**
   * 页面底颜色（按纵向位置在三段渐变上插值）。
   * 这是**最坏情况**的兜底：真正要看的底是元素自己的背景链。
   */
  const pageBgAt = (el) => {
    const r = el.getBoundingClientRect();
    const t = Math.min(1, Math.max(0, (r.top + r.height / 2) / window.innerHeight));
    const stops = dark
      ? [[0, [59, 43, 82]], [0.55, [39, 29, 55]], [1, [18, 14, 28]]]
      : [[0, [250, 248, 245]], [0.55, [240, 236, 230]], [1, [221, 215, 205]]];
    let a = stops[0], c = stops[2];
    for (let i = 0; i < 2; i++)
      if (t >= stops[i][0] && t <= stops[i + 1][0]) {
        a = stops[i];
        c = stops[i + 1];
        break;
      }
    const k = (t - a[0]) / (c[0] - a[0] || 1);
    return [0, 1, 2].map((i) => Math.round(a[1][i] + (c[1][i] - a[1][i]) * k));
  };

  /**
   * 元素的**有效**背景：向上遍历祖先，把每层的 background-color 按 alpha 叠起来。
   *
   * 为什么必须这么做（这是这个审计脚本最大的坑）：
   *   旧版只会用“页面底颜色”作比较底。于是一颗**金底墨字**的主按钮
   *   会被算成“墨字落在深色页面底上” → 报告 1.2:1（假阳性）。
   *   实测一轮下来 10 条里有 6 条是这类误报，真正的可读性问题被淹在噪音里。
   *   现在：按钮自己（或它下面某层）有实心底 → 叠加它，只算真正的"浮在背景上"的文字。
   */
  const effectiveBg = (el) => {
    const chain = [];
    let node = el;
    while (node && node !== document.documentElement) {
      const cs = getComputedStyle(node);
      const bg = parse(cs.backgroundColor);
      if (bg && bg.a > 0.02) chain.push(bg);
      // 完全不透明的底已经定死了，不用再往上找
      if (bg && bg.a >= 0.98) break;
      node = node.parentElement;
    }
    // 从最外层（页面底）开始向内叠加
    let base = pageBgAt(el);
    for (let i = chain.length - 1; i >= 0; i--) {
      const c = chain[i];
      base = [
        Math.round(c.r * c.a + base[0] * (1 - c.a)),
        Math.round(c.g * c.a + base[1] * (1 - c.a)),
        Math.round(c.b * c.a + base[2] * (1 - c.a)),
      ];
    }
    return base;
  };

  const host = document.querySelector("#probe-light, #probe-dark");
  if (!host) return { error: "未挂载" };
  const bad = [];
  for (const el of host.querySelectorAll("*")) {
    const hasText = [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
    if (!hasText) continue;
    const cs = getComputedStyle(el);
    if (cs.visibility === "hidden" || cs.display === "none" || +cs.opacity === 0) continue;
    // 禁用控件跳过：WCAG 1.4.3 / 1.4.11 明确豁免“不可用组件”。
    // 不跳的话，每个 disabled 按钮都会成为一条假阳性（实测一次能冲到 25% 的条目）。
    if (el.closest("[disabled], [aria-disabled='true']")) continue;
    // emoji / 封面占位 / 覆盖在封面上的文字：不按页面底算对比度
    // （它们实际叠在图片或黑色渐变遮罩上，底色不是页面底）
    if (
      el.className?.toString().match(
        /emoji|mh-emo|mh-dot|tile-placeholder|tile-badge|tile-check|tile-time|tile-cap|smart-subicon|scan-subicon/,
      )
    )
      continue;
    // 祖先里有 .tile-cap（自带深色胶囊底）的元素同样跳过
    if (el.closest(".tile-cap")) continue;
    const fg = parse(cs.color);
    if (!fg) continue;
    const bg = effectiveBg(el);
    const comp = [
      Math.round(fg.r * fg.a + bg[0] * (1 - fg.a)),
      Math.round(fg.g * fg.a + bg[1] * (1 - fg.a)),
      Math.round(fg.b * fg.a + bg[2] * (1 - fg.a)),
    ];
    const cr = ratio(comp, bg);
    if (cr < 4.5) {
      bad.push({
        文本: el.textContent.trim().slice(0, 12),
        类: el.className?.toString().split(" ")[0],
        字号: cs.fontSize,
        对比度: Number(cr.toFixed(2)),
      });
    }
  }
  return { 预设: window.__PRESET__, 不合格数: bad.length, 明细: bad.slice(0, 10) };
};

const PAGES = process.argv.slice(2).length
  ? process.argv.slice(2)
  : ["AlbumList", "SmartAlbum", "ScanHub", "Memories", "Timeline", "Workshop"];

const PORT = process.env.VITE_PORT || "1420";

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const summary = [];
  for (const page of PAGES) {
    for (const mode of ["light", "dark"]) {
      const p = await b.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });
      const errs = [];
      p.on("pageerror", (e) => errs.push(String(e).split("\n")[0]));
      await p.goto(`http://localhost:${PORT}/.visual/themed-probe.html#${mode}/${page}`, { waitUntil: "load" });
      await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 25000 }).catch(() => {});
      await p.evaluate(() => document.fonts.ready);
      await p.waitForTimeout(800);

      const r = await p.evaluate(AUDIT);
      summary.push({ page, mode, 不合格: r.不合格数 ?? "?", 错误: errs[0] ?? "" }); if (r.明细?.length) console.log(`\n[${page}/${mode}]`, JSON.stringify(r.明细.slice(0,6)));

      if (mode === "light" && !errs.length) {
        const info = await p.evaluate((mode) => {
          const el = document.getElementById(`probe-${mode}`);
          if (!el) return null;
          const rc = el.getBoundingClientRect();
          return { top: rc.top + window.scrollY, height: rc.height };
        }, mode);
        if (info && info.height > 0) {
          await p.screenshot({
            path: `design/preview/v4-${page}-light.png`,
            clip: { x: 0, y: info.top, width: 1440, height: Math.min(info.height, 780) },
            fullPage: true,
          });
        }
      }
      await p.close();
    }
  }

  console.log("\n页面".padEnd(14), "模式".padEnd(7), "不合格数");
  for (const s of summary) {
    console.log(String(s.page).padEnd(14), String(s.mode).padEnd(7), s.不合格, s.错误 ? `  ⚠ ${s.错误}` : "");
  }
  await b.close();
})();

