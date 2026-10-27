// 液态玻璃材质探针 v2（真实 UI / 像素级判据）
//
// 判据不是「看起来像不像玻璃」，而是三条可量的东西：
//   ① 板面 vs 板外背景的**明度差** —— 浅底上看不出玻璃的直接原因就是它 ≈ 0；
//   ② 板缘亮度跳变（上缘 vs 下缘）—— 板厚能不能被读出；
//   ③ 拖透明度滑块时画面变化的像素占比 —— 「改透明度没变化」的直接判据。
//
// 用法：先起 vite dev（1420），再 node .visual/probe-liquid2.mjs
import { chromium } from "playwright";
import { mkdirSync } from "node:fs";
import { PNG } from "pngjs";

const PORT = process.env.VITE_PORT || "1420";
const OUT = ".visual/baseline/liquid";
mkdirSync(OUT, { recursive: true });

const PREFS = {
  light: { mode: "light", compColor: "#eef4f1", compAlpha: 0.72 },
  dark: { mode: "dark", compColor: "#2b3037", compAlpha: 0.62 },
};

const lum = ([r, g, b]) => {
  const f = (v) => {
    v /= 255;
    return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
};

const browser = await chromium.launch();

async function shot(mode, patch, name, route = "/home") {
  const prefs = { ...PREFS[mode], ...patch, bgStyle: "gradient" };
  if (mode === "light") {
    Object.assign(prefs, { gradFrom: "#ecf3f0", gradMid: "#d6e1dc", gradTo: "#a6b9b2", gradAngle: 165 });
  } else {
    Object.assign(prefs, { gradFrom: "#0b0a0d", gradMid: "#1b2320", gradTo: "#2f3a35", gradAngle: 165 });
  }
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  const page = await ctx.newPage();
  page.on("pageerror", (e) => console.log("pageerror:", String(e).split("\n")[0]));
  await page.addInitScript((p) => localStorage.setItem("pm-theme", JSON.stringify(p)), prefs);
  const q = new URLSearchParams({ mat: prefs.material, mode, alpha: String(prefs.compAlpha) });
  await page.goto(`http://localhost:${PORT}/.visual/preview.html?${q}#${route}`, { waitUntil: "load" });
  await page.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 30000 }).catch(() => {});
  await page.waitForTimeout(1000);
  const boxes = await page.evaluate(() =>
    [...document.querySelectorAll(".glass-card, .glass-surface")]
      .filter((e) => e.getBoundingClientRect().width > 120)
      .slice(0, 3)
      .map((e) => {
        const r = e.getBoundingClientRect();
        return { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) };
      }),
  );
  const buf = await page.screenshot({ path: `${OUT}/${name}.png` });
  const extra = await page.evaluate(() => {
    const el = document.querySelector(".glass-card");
    if (!el) return null;
    const cs = getComputedStyle(el);
    const a = getComputedStyle(el, "::after");
    return {
      fill: cs.backgroundColor,
      backdrop: cs.backdropFilter,
      afterContent: a.content,
      afterMask: a.maskComposite || a.webkitMaskComposite || "",
      shadow: cs.boxShadow.slice(0, 90),
    };
  });
  await ctx.close();
  return { buf, boxes, extra };
}

const px = (img, x, y) => {
  const i = (y * img.width + x) * 4;
  return [img.data[i], img.data[i + 1], img.data[i + 2]];
};
/** 板面 vs 板外：同一水平线上，板内取中心点、板外取左侧 12px 处 */
function boardVsBg(img, box) {
  const y = Math.round(box.y + box.h / 2);
  const inside = px(img, Math.round(box.x + box.w / 2), y);
  const outside = px(img, Math.max(0, box.x - 14), y);
  const li = lum(inside);
  const lo = lum(outside);
  return {
    板面: inside.join(","),
    板外: outside.join(","),
    明度差: +(Math.abs(li - lo) * 100).toFixed(1) + "%",
  };
}

const diffPct = (A, B) => {
  let n = 0;
  const total = A.data.length / 4;
  for (let i = 0; i < A.data.length; i += 4) {
    const d = Math.abs(A.data[i] - B.data[i]) + Math.abs(A.data[i + 1] - B.data[i + 1]) + Math.abs(A.data[i + 2] - B.data[i + 2]);
    if (d > 12) n++;
  }
  return +((n / total) * 100).toFixed(2);
};

const out = {};
for (const mode of ["light", "dark"]) {
  const plain = await shot(mode, { material: "plain" }, `${mode}-plain`);
  const liquid = await shot(mode, { material: "liquid" }, `${mode}-liquid`);
  const low = await shot(mode, { material: "liquid", compAlpha: 0.25 }, `${mode}-liquid-a25`);
  const mid = await shot(mode, { material: "liquid", compAlpha: 0.5 }, `${mode}-liquid-a50`);

  const P = PNG.sync.read(plain.buf);
  const L = PNG.sync.read(liquid.buf);
  const Lo = PNG.sync.read(low.buf);
  const M = PNG.sync.read(mid.buf);

  out[mode] = {
    "液态 computed": liquid.extra,
    板面vs板外: liquid.boxes.map((b) => boardVsBg(L, b)),
    "plain 板面vs板外": plain.boxes.map((b) => boardVsBg(P, b)),
    "plain→liquid 变化像素%": diffPct(P, L),
    "alpha 25%→50% 变化像素%": diffPct(Lo, M),
    "alpha 50%→72%(默认) 变化像素%": diffPct(M, L),
    "alpha25 板面": low.boxes.slice(0, 1).map((b) => boardVsBg(Lo, b)),
  };
}
console.log(JSON.stringify(out, null, 1));
await browser.close();
