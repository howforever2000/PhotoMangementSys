/**
 * 视觉验收 · 机测版 v2（不依赖截图肉眼）
 *
 * v1 的盲区：渐变背景用 backgroundColor 读不到，且文字下方的「黑色 scrim 遮罩」
 * 是兄弟节点而非祖先，量不到 → 会把「有黑纱保护的白字」误判为不达标。
 *
 * v2 做法（真实像素语义）：
 *   1. 沿祖先链自底向上收集背景层（含 background-image 的每一层，层序按 CSS 规范）；
 *   2. 另外收集「覆盖型兄弟遮罩」（absolute/fixed 且覆盖祖先 ≥80% 的子树，DOM 早于文字链者）；
 *   3. 渐变按**取样点**解析线性插值（135deg / 180deg 等），不是取最亮色块；
 *   4. 文字矩形取 3 个采样点，取最不利的一点（浅字取最亮、深字取最暗）算对比度。
 *
 * 判定：小字/常规字 ≥ 4.5:1，大字（≥24px 或 ≥18.66px 加粗）≥ 3:1。
 * 豁免：禁用控件（:disabled / [aria-disabled] / .module-pending）按 WCAG 1.4.3 不计。
 */
const { chromium } = require("playwright");
const path = require("path");
const fs = require("fs");

const { INIT } = require("./mock-init.cjs");

const ROUTES = ["/home", "/albums", "/scan", "/smart", "/memories", "/album/1"];

const AUDIT = () => {
  /* ---------- 颜色基础 ---------- */
  const parse = (c) => {
    if (!c) return null;
    if (c === "transparent") return { r: 0, g: 0, b: 0, a: 0 };
    const m = c.match(/rgba?\(([^)]+)\)/);
    if (!m) {
      if (/^#[0-9a-f]{3,8}$/i.test(c)) {
        let h = c.slice(1);
        if (h.length === 3) h = h.split("").map((s) => s + s).join("");
        const n = parseInt(h.slice(0, 6), 16);
        return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255, a: 1 };
      }
      return null;
    }
    const p = m[1].split(",").map((s) => parseFloat(s));
    return { r: p[0], g: p[1], b: p[2], a: p.length > 3 ? p[3] : 1 };
  };
  const lin = (v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  const lum = (c) => 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b);
  const over = (fg, bg) => ({
    r: fg.r * fg.a + bg.r * (1 - fg.a),
    g: fg.g * fg.a + bg.g * (1 - fg.a),
    b: fg.b * fg.a + bg.b * (1 - fg.a),
    a: fg.a + bg.a * (1 - fg.a),
  });
  const ratio = (a, b) => {
    const la = lum(a), lb = lum(b);
    return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
  };
  const mix = (a, b, t) => ({
    r: a.r + (b.r - a.r) * t,
    g: a.g + (b.g - a.g) * t,
    b: a.b + (b.b - a.b) * t,
    a: a.a + (b.a - a.a) * t,
  });

  /* ---------- 按顶层逗号拆分（rgba 内的逗号不算） ---------- */
  const splitTop = (s) => {
    const out = [];
    let depth = 0, cur = "";
    for (const ch of s) {
      if (ch === "(") depth++;
      if (ch === ")") depth--;
      if (ch === "," && depth === 0) { out.push(cur); cur = ""; continue; }
      cur += ch;
    }
    if (cur.trim()) out.push(cur);
    return out.map((x) => x.trim()).filter(Boolean);
  };

  /** 线性渐变在 (x,y) 处的真实颜色；无法解析时回退到最亮色块（保守） */
  const gradColorAt = (layer, rect, x, y) => {
    const inner = layer.replace(/^\s*linear-gradient\(/i, "").replace(/\)\s*$/, "");
    const parts = splitTop(inner);
    if (!parts.length) return null;
    let angle = 180, i0 = 0;
    const head = parts[0].toLowerCase();
    if (/^[0-9.]+deg$/.test(head)) { angle = parseFloat(head); i0 = 1; }
    else if (head.startsWith("to ")) {
      angle = { "to top": 0, "to right": 90, "to bottom": 180, "to left": 270 }[head] ?? 180;
      i0 = 1;
    }
    const stops = [];
    for (let i = i0; i < parts.length; i++) {
      const raw = parts[i].trim();
      const pm = raw.match(/\s+([0-9.]+)%\s*$/);
      let colorStr = raw, pos = null;
      if (pm) { pos = parseFloat(pm[1]) / 100; colorStr = raw.slice(0, pm.index).trim(); }
      const col = parse(colorStr);
      if (!col) continue;
      stops.push({ col, pos });
    }
    if (!stops.length) return null;
    stops.forEach((s, i) => {
      s.pos = s.pos ?? (i === 0 ? 0 : i === stops.length - 1 ? 1 : i / (stops.length - 1));
    });
    if (!/linear-gradient/i.test(layer)) {
      // 径向等：取最亮色块（对白字最不利，保守）
      return stops.reduce((a, b) => (lum(b.col) > lum(a.col) ? b : a)).col;
    }
    const a = (angle * Math.PI) / 180;
    const dx = Math.sin(a), dy = -Math.cos(a);
    const W = rect.width, H = rect.height;
    const L = Math.abs(W * dx) + Math.abs(H * dy) || 1;
    const sx = rect.left + W / 2 - (dx * L) / 2;
    const sy = rect.top + H / 2 - (dy * L) / 2;
    let t = ((x - sx) * dx + (y - sy) * dy) / L;
    t = Math.max(0, Math.min(1, t));
    let s0 = stops[0], s1 = stops[stops.length - 1];
    for (let i = 0; i < stops.length - 1; i++) {
      if (t >= stops[i].pos && t <= stops[i + 1].pos) { s0 = stops[i]; s1 = stops[i + 1]; break; }
    }
    const span = s1.pos - s0.pos || 1;
    return mix(s0.col, s1.col, (t - s0.pos) / span);
  };

  /** 节点自身的背景层，按「自底向上」返回：先 background-color，再 image 层（末层在下） */
  const layersOf = (node) => {
    const cs = getComputedStyle(node);
    const out = [];
    const bgc = parse(cs.backgroundColor);
    if (bgc && bgc.a > 0) out.push({ color: bgc });
    const img = cs.backgroundImage || "";
    if (img && img.includes("gradient")) {
      const grads = splitTop(img).filter((g) => g.includes("gradient"));
      for (let i = grads.length - 1; i >= 0; i--) out.push({ grad: grads[i] });
    }
    return out;
  };

  const isCovering = (child, box) => {
    if (!/^(absolute|fixed)$/.test(getComputedStyle(child).position)) return false;
    const r = child.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return false;
    return r.left <= box.left + 4 && r.top <= box.top + 4 &&
      r.right >= box.right - 4 && r.bottom >= box.bottom - 4;
  };

  /** 文字下方的真实底色：祖先链 + 覆盖型兄弟遮罩，按绘制顺序合成 */
  const bgAt = (el, x, y) => {
    const chain = [];
    for (let n = el; n; n = n.parentElement) chain.push(n);
    const inChain = new Set(chain);
    const paints = []; // 自底向上：{layer, rect}
    const pushNode = (node) => {
      const r = node.getBoundingClientRect();
      for (const l of layersOf(node)) paints.push({ ...l, rect: r });
    };
    for (let i = chain.length - 1; i >= 0; i--) {
      const n = chain[i];
      pushNode(n);
      const box = n.getBoundingClientRect();
      // 覆盖型遮罩（如 .mem-story-fade）：压在文字下方、但不是祖先
      for (const c of n.children) {
        if (inChain.has(c)) continue;
        if (!isCovering(c, box)) continue;
        const stack = [c];
        while (stack.length) {
          const s = stack.shift();
          if (inChain.has(s)) continue;
          pushNode(s);
          stack.unshift(...s.children);
        }
      }
    }
    let cur = null;
    for (const p of paints) {
      const c = p.grad ? gradColorAt(p.grad, p.rect, x, y) : p.color;
      if (!c) continue;
      cur = cur ? over(c, cur) : c;
    }
    if (!cur) return { r: 14, g: 33, b: 27, a: 1 };
    return cur.a < 1 ? over(cur, { r: 14, g: 33, b: 27, a: 1 }) : cur;
  };

  /* ---------- 逐元素判定 ---------- */
  const fails = [];
  const els = document.querySelectorAll("body *");
  for (const el of els) {
    const hasOwnText = [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
    if (!hasOwnText) continue;
    const text = (el.textContent || "").trim();
    if (!text || text.length > 120) continue;
    const cs = getComputedStyle(el);
    if (cs.visibility === "hidden" || cs.display === "none" || parseFloat(cs.opacity) < 0.5) continue;
    if (el.matches(":disabled") || el.closest(":disabled, .module-pending, [aria-disabled='true']")) continue;
    const rect = el.getBoundingClientRect();
    if (rect.width < 4 || rect.height < 4) continue;
    const fg = parse(cs.color);
    if (!fg) continue;

    const darkText = lum({ r: fg.r, g: fg.g, b: fg.b }) < 0.4;
    const pts = [
      [rect.left + Math.min(24, rect.width * 0.2), rect.top + rect.height / 2],
      [rect.left + rect.width / 2, rect.top + rect.height / 2],
      [rect.right - 6, rect.top + rect.height / 2],
    ];
    let worst = null, worstR = Infinity;
    for (const [px, py] of pts) {
      const bg = bgAt(el, px, py);
      const fgOver = fg.a < 1 ? over(fg, bg) : fg;
      const r = ratio(fgOver, bg);
      if (r < worstR) { worstR = r; worst = bg; }
    }

    const size = parseFloat(cs.fontSize);
    const bold = parseInt(cs.fontWeight, 10) >= 700;
    const large = size >= 24 || (bold && size >= 18.66);
    const need = large ? 3 : 4.5;
    if (worstR < need) {
      fails.push({
        text: text.slice(0, 40),
        cls: (el.className || "").toString().slice(0, 40),
        color: cs.color,
        bg: worst ? `rgb(${Math.round(worst.r)},${Math.round(worst.g)},${Math.round(worst.b)})` : "?",
        ratio: Math.round(worstR * 100) / 100,
        need,
        size,
      });
    }
  }

  /* ---------- 关键结构探针 ---------- */
  const probe = (sel) => {
    const el = document.querySelector(sel);
    if (!el) return null;
    const s = getComputedStyle(el);
    return {
      sel,
      background: s.backgroundColor,
      backgroundImage: (s.backgroundImage || "").slice(0, 70),
      color: s.color,
      radius: s.borderRadius,
      shadow: s.boxShadow.slice(0, 50),
      filter: (s.backdropFilter || "").slice(0, 50),
    };
  };
  const body = getComputedStyle(document.body);
  /** 抽样证明：这几个文字点位下面真实合成出来的底色是什么 */
  const sampleOf = (sel) => {
    const el = document.querySelector(sel);
    if (!el) return null;
    const r = el.getBoundingClientRect();
    const bg = bgAt(el, r.left + r.width / 2, r.top + r.height / 2);
    const fg = parse(getComputedStyle(el).color);
    return {
      sel,
      color: getComputedStyle(el).color,
      bg: `rgb(${Math.round(bg.r)},${Math.round(bg.g)},${Math.round(bg.b)})`,
      ratio: Math.round(ratio(fg, bg) * 100) / 100,
    };
  };
  return {
    url: location.pathname,
    samples: [
      sampleOf(".scan-hero-title"), sampleOf(".scan-subcard .scan-subtitle-line"),
      sampleOf(".mem-story-title"), sampleOf(".mem-year-title"), sampleOf(".smart-subcard .smart-subtitle-line"),
    ].filter(Boolean),
    tokens: {
      glassBg: body.getPropertyValue("--glass-bg").trim(),
      surface: body.getPropertyValue("--color-surface").trim(),
      text: body.getPropertyValue("--color-text").trim(),
      shadow1: body.getPropertyValue("--shadow-1").trim(),
      radiusCard: body.getPropertyValue("--radius-card").trim(),
      darkClass: document.body.classList.contains("theme-dark"),
    },
    probes: [
      probe(".scan-hero"), probe(".scan-subcard"), probe(".smart-subcard"),
      probe(".module-card"), probe(".album-card"), probe(".mem-story"), probe(".collapse-section"),
    ].filter(Boolean),
    failCount: fails.length,
    fails: fails.slice(0, 14),
  };
};

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  for (const route of ROUTES) {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const page = await ctx.newPage();
    await page.addInitScript(INIT);
    await page.goto(`http://localhost:1420${route}`, { waitUntil: "networkidle", timeout: 20000 }).catch(() => {});
    await page.waitForTimeout(1200);
    const res = await page.evaluate(AUDIT).catch((e) => ({ url: route, error: String(e) }));
    console.log(JSON.stringify(res));
    await ctx.close();
  }
  await browser.close();
})();

