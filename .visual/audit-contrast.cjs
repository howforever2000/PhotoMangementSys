// 多页 × 深浅的对比度审计 + 截图
const { chromium } = require("playwright");

const AUDIT = () => {
  const parse = (s) => {
    const m = (s || "").match(/[\d.]+/g);
    if (!m) return null;
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
  const bgAt = (el) => {
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

  const host = document.querySelector("#probe-light, #probe-dark");
  if (!host) return { error: "未挂载" };
  const bad = [];
  for (const el of host.querySelectorAll("*")) {
    const hasText = [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
    if (!hasText) continue;
    const cs = getComputedStyle(el);
    if (cs.visibility === "hidden" || cs.display === "none" || +cs.opacity === 0) continue;
    // emoji / 封面占位 / 覆盖在封面上的徽标：不按页面底算对比度
    if (el.className?.toString().match(/emoji|mh-emo|mh-dot|tile-placeholder|tile-badge|tile-check|smart-subicon|scan-subicon/)) continue;
    const fg = parse(cs.color);
    if (!fg) continue;
    const bg = bgAt(el);
    const comp = [
      fg.r * fg.a + bg[0] * (1 - fg.a),
      fg.g * fg.a + bg[1] * (1 - fg.a),
      fg.b * fg.a + bg[2] * (1 - fg.a),
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

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const summary = [];
  for (const page of PAGES) {
    for (const mode of ["light", "dark"]) {
      const p = await b.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });
      const errs = [];
      p.on("pageerror", (e) => errs.push(String(e).split("\n")[0]));
      await p.goto(`http://localhost:5173/.visual/themed-probe.html#${mode}/${page}`, { waitUntil: "load" });
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

