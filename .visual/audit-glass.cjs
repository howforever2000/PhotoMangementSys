// 审计玻璃板上的文字：板底 = 玻璃填充叠在页面底上
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
  const over = (fg, bg) => [
    fg.r * fg.a + bg[0] * (1 - fg.a),
    fg.g * fg.a + bg[1] * (1 - fg.a),
    fg.b * fg.a + bg[2] * (1 - fg.a),
  ];

  const dark = document.body.classList.contains("theme-dark");
  /* 页面底：从真实 DOM 取（探针把三段渐变铺在 host 上），
     不能硬编码 —— 不同预设的渐变不同，算出来的板底就不对，
     会出现“浅色下瓦片名 1.09:1”这种明显荒谬的结果。 */
  const hostEl = document.querySelector("#probe-dark, #probe-light");
  const pageBg = (() => {
    if (!hostEl) return dark ? [39, 29, 55] : [240, 236, 230];
    // 渐变三色的平均（浅色预设纸白 #faf8f5/#f0ece6/#ddd7cd）
    const m = getComputedStyle(hostEl).backgroundImage.match(/[\d.]+/g);
    if (!m) return dark ? [39, 29, 55] : [240, 236, 230];
    const stops = [];
    for (let i = 0; i + 2 < m.length; i += 3) stops.push([+m[i], +m[i + 1], +m[i + 2]]);
    if (!stops.length) return dark ? [39, 29, 55] : [240, 236, 230];
    const avg = [0, 1, 2].map((c) => Math.round(stops.reduce((s, p) => s + p[c], 0) / stops.length));
    return avg;
  })();
  const glassFill = parse(getComputedStyle(document.body).getPropertyValue("--panel-glass-fill"));

  const host = hostEl;
  if (!host || !glassFill) return { error: "未就绪" };

  // 玻璃板底 = 玻璃填充叠在页面底上
  const panelBg = over(glassFill, pageBg);

  const check = (name, sel) => {
    const el = host.querySelector(sel);
    if (!el) return null;
    const cs = getComputedStyle(el);
    const fg = parse(cs.color);
    if (!fg) return null;
    return {
      名称: name,
      文字: cs.color,
      字号: cs.fontSize,
      对比度: Number(ratio(over(fg, panelBg), panelBg).toFixed(2)),
    };
  };

  const items = [
    check("数据带标签", ".u-stat-label"),
    check("数据带数字", ".u-statband .num"),
    check("数据带单位", ".u-statband .unit"),
    check("数据带 delta", ".u-statband .delta"),
    check("左栏分组标题", ".side-h"),
    check("左栏项", ".side-item"),
    check("左栏计数", ".side-n"),
    check("瓦片名", ".tile-name"),
    check("瓦片元信息", ".tile-meta"),
    check("顶栏组件名", ".mh-title"),
    check("顶栏读数", ".mh-meta"),
    check("顶栏分区", ".mh-nav-item"),
  ].filter(Boolean);

  return { 玻璃填充: getComputedStyle(document.body).getPropertyValue("--panel-glass-fill").trim(), 板底: panelBg.map(Math.round), 明细: items };
};

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  for (const mode of ["dark", "light"]) {
    const p = await b.newPage({ viewport: { width: 1440, height: 1000 } });
    await p.goto(`http://localhost:5173/.visual/themed-probe.html#${mode}/AlbumList`, { waitUntil: "load" });
    await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 25000 }).catch(() => {});
    await p.waitForTimeout(800);
    const r = await p.evaluate(AUDIT);
    console.log(`\n===== ${mode} =====`);
    console.log("玻璃填充:", r.玻璃填充, " 板底:", r.板底?.join(","));
    for (const it of r.明细 ?? []) {
      const flag = it.对比度 >= 4.5 ? "✅" : it.对比度 >= 3 ? "⚠️" : "❌";
      console.log(String(it.名称).padEnd(14), String(it.字号).padEnd(8), `${flag} ${it.对比度}:1`);
    }
    await p.close();
  }
  await b.close();
})();
