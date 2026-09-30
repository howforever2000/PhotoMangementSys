/**
 * 渲染像素版对比度审计（FEAT-094）
 *
 * 为什么需要它：audit-contrast.cjs 按 CSS 推算底色，**看不到 background-image: url(...)**
 * （照片/壁纸背景）——推算时会退回兜底深色，于是「背景图模式」下它给出的 0 失败是**假通过**。
 * 本脚本改为量真实渲染像素：截图 → 把 PNG 作为 data URL 送回页面 → canvas 解码 → 读像素。
 *
 * 取样方法：对每个含文字的元素，在其盒子内部按网格取点，用**分位数**取「最不利的背景」——
 * 浅色文字取亮度 P90（盒内最亮的一片），深色文字取 P10。盒内大部分像素是底色、少数是字墨，
 * 取分位既避开字墨又抓到亮/暗块，不依赖「我知道那里有什么」。
 *
 * 用法：
 *   node .impeccable/audit-contrast-pixels.cjs
 *   PM_PIX_ROUTES=/home,/albums PM_AUDIT_ALPHA=0 node .impeccable/audit-contrast-pixels.cjs
 */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

const ROUTES = (process.env.PM_PIX_ROUTES || "/home,/albums,/scan,/album/1").split(",").map((s) => s.trim());
/** 覆盖偏好：PM_AUDIT_PREFS='{"compAlpha":0}'（会经 normalizePrefs 补齐其余字段） */
const PREFS_JSON = process.env.PM_AUDIT_PREFS;
const ALPHA = process.env.PM_AUDIT_ALPHA;

const AUDIT = async (shot) => {
  const lin = (v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  const lum = (c) => 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
  const ratio = (a, b) => {
    const la = lum(a), lb = lum(b);
    return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
  };
  const parse = (c) => {
    const m = (c || "").match(/rgba?\(([^)]+)\)/);
    if (!m) return null;
    const p = m[1].split(",").map((s) => parseFloat(s));
    return [p[0], p[1], p[2], p.length > 3 ? p[3] : 1];
  };

  const img = new Image();
  img.src = shot;
  await img.decode();
  const cv = document.createElement("canvas");
  cv.width = img.naturalWidth;
  cv.height = img.naturalHeight;
  const ctx = cv.getContext("2d", { willReadFrequently: true });
  ctx.drawImage(img, 0, 0);
  const data = ctx.getImageData(0, 0, cv.width, cv.height).data;
  const sx = cv.width / window.innerWidth;
  const sy = cv.height / window.innerHeight;
  const at = (x, y) => {
    const px = Math.round(x * sx), py = Math.round(y * sy);
    if (px < 0 || py < 0 || px >= cv.width || py >= cv.height) return null;
    const i = (py * cv.width + px) * 4;
    return [data[i], data[i + 1], data[i + 2]];
  };

  const out = [];
  for (const el of document.querySelectorAll("body *")) {
    if (![...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim())) continue;
    const cs = getComputedStyle(el);
    if (cs.visibility === "hidden" || cs.display === "none" || parseFloat(cs.opacity) < 0.5) continue;
    if (el.matches(":disabled") || el.closest(":disabled, [aria-disabled='true']")) continue;
    const r = el.getBoundingClientRect();
    if (r.width < 6 || r.height < 6) continue;
    if (r.bottom < 0 || r.top > window.innerHeight || r.right < 0 || r.left > window.innerWidth) continue;
    const fg = parse(cs.color);
    if (!fg || fg[3] < 0.5) continue;

    /* 取样规则（与 verify-auth.cjs 同一套，已踩过两个坑）：
       ① 元素自带底色（按钮/输入框/芯片）→ 文字压在自己的填充上，只取**框内**贴边、避开墨迹的点；
          混入框外样本会把「金底墨字」量成压在背景上的假失败。
       ② 元素无底色（标题/标签/链接）→ 只取**框外**贴边的点；框内取样会点到墨迹（大字尤重）。
       ③ 无论哪类，都丢弃「颜色接近文字色」的样本（抗锯齿边缘/相邻文字）。 */
    const ownBg = parse(cs.backgroundColor);
    const hasOwnBg = !!ownBg && ownBg[3] > 0.02;

    /* ④ 把取样点裁剪到「最近一个会画底的祖先」——否则贴边 6px 会跑到卡片外的
       页面底色上，拿背景去和卡内文字比 → 假失败（英雄卡内的大数字就这么被误报过）。 */
    let bound = { left: 0, top: 0, right: window.innerWidth, bottom: window.innerHeight };
    for (let n = el; n; n = n.parentElement) {
      const ns = getComputedStyle(n);
      const nb = parse(ns.backgroundColor);
      const hasPaint = (!!nb && nb[3] > 0.02) || (ns.backgroundImage && ns.backgroundImage !== "none");
      if (hasPaint) {
        const b = n.getBoundingClientRect();
        bound = { left: b.left + 2, top: b.top + 2, right: b.right - 2, bottom: b.bottom - 2 };
        break;
      }
    }
    const pad = 6;
    const cand = hasOwnBg
      ? [
          [r.left + 4, r.top + r.height / 2], [r.right - 4, r.top + r.height / 2],
          [r.left + r.width / 2, r.top + 3], [r.left + r.width / 2, r.bottom - 3],
        ]
      : [
          [r.left - pad, r.top + r.height / 2], [r.right + pad, r.top + r.height / 2],
          [r.left + r.width / 2, r.top - pad], [r.left + r.width / 2, r.bottom + pad],
          [r.left - pad, r.top - pad], [r.right + pad, r.top - pad],
          [r.left - pad, r.bottom + pad], [r.right + pad, r.bottom + pad],
        ];
    const samples = [];
    for (const [x0, y0] of cand) {
      // ④ 裁剪：落在「最近会画底的祖先」之外的点一律丢弃（不拿背景去比卡内文字）
      const x = Math.min(Math.max(x0, bound.left), bound.right);
      const y = Math.min(Math.max(y0, bound.top), bound.bottom);
      if (hasOwnBg && (x < r.left || x > r.right || y < r.top || y > r.bottom)) continue;
      if (x < 1 || y < 1 || x >= window.innerWidth - 1 || y >= window.innerHeight - 1) continue;
      const c = at(x, y);
      if (!c) continue;
      // ③ 剔除与文字色接近的样本（字墨 / 抗锯齿）
      if (Math.abs(c[0] - fg[0]) + Math.abs(c[1] - fg[1]) + Math.abs(c[2] - fg[2]) < 40) continue;
      samples.push(c);
    }
    if (!samples.length) continue; // 取不到可信背景样本 → 不判定（宁可不报，也不报假失败）
    const lightText = lum(fg) > 0.4;
    const pick = samples.reduce((a, b) => (lightText ? (lum(b) > lum(a) ? b : a) : lum(b) < lum(a) ? b : a));
    const cr = ratio(fg, pick);
    const size = parseFloat(cs.fontSize);
    const bold = parseInt(cs.fontWeight, 10) >= 700;
    const need = size >= 24 || (bold && size >= 18.66) ? 3 : 4.5;
    if (cr < need) {
      out.push({
        text: (el.textContent || "").trim().slice(0, 26),
        cls: (el.className || "").toString().slice(0, 30),
        color: cs.color,
        worstBg: `rgb(${pick.join(",")})`,
        ratio: Math.round(cr * 100) / 100,
        need,
        size,
        y: Math.round(r.top),
      });
    }
  }
  return out;
};

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  let total = 0;
  for (const route of ROUTES) {
    const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } });
    const page = await ctx.newPage();
    await page.addInitScript(INIT);
    const prefsOverride = PREFS_JSON !== undefined ? PREFS_JSON : ALPHA !== undefined ? JSON.stringify({ compAlpha: Number(ALPHA) }) : null;
    if (prefsOverride) {
      await page.addInitScript((p) => localStorage.setItem("pm-theme", p), prefsOverride);
    }
    await page.goto(`http://localhost:1420${route}`, { waitUntil: "networkidle", timeout: 20000 }).catch(() => {});
    await page.waitForTimeout(1200);
    const shot = (await page.screenshot()).toString("base64");
    const fails = await page.evaluate(AUDIT, "data:image/png;base64," + shot).catch((e) => [{ text: "AUDIT_ERROR " + e.message }]);
    total += fails.length;
    console.log(`\n${route}  偏好=${prefsOverride || "默认"}  不达标 ${fails.length} 项`);
    for (const f of fails.slice(0, 12)) {
      console.log(`   ✘ ${String(f.ratio).padStart(5)}:1 (需${f.need}) y=${f.y} ${f.size}px  ${f.text}  <${f.cls}> ${f.color} on ${f.worstBg}`);
    }
    await ctx.close();
  }
  console.log(`\n合计不达标 ${total} 项`);
  await browser.close();
})();
