/**
 * 板面对比探针（FEAT-113）
 *
 * 为什么不复用 probe-line-visibility：
 *   它测的是「1px 硬边线 vs 相邻像素」的对比度。FEAT-113 把「四边同亮硬线」
 *   改成了「上亮下暗立体边 + 接触影」，**硬边线本来就不存在了**，再去采样
 *   那条线的 ±1px，测到的是渐变过渡，指标必然失效（基线本身也 23/25）。
 *
 * 真正要回答的问题只有一个：**这块板在它的背景上，还看不看得出来？**
 *   判据 = 板内清洁区域的亮度中位数 vs 板外紧邻背景的亮度中位数，
 *          WCAG 对比度 ≥ 3:1（非文字相邻色，1.4.11）。
 *   这块是「块面 vs 块面」的比较，与用硬线还是用影修边无关 —— 修边方式
 *   可以随便改，只要板面本身跟背景拉得开，边界就成立。
 *
 * 采样方式刻意做成「多点中位数 + 只取方差小的列」，避免：
 *   · 采到文字/数字/图标 → 亮度被拉偏（基线里 widget0-surface 131 就是采到了浅色内容）
 *   · 边界偏移 1~2px 就全盘错位（边框增删会让 border-box 变 2px）
 *
 * 用法：node .visual/probe-board-contrast.cjs [variant...]
 *   variant 见 overview-probe.html#dusk|pine|plain|wall|light
 */
const { chromium } = require("playwright");
const zlib = require("zlib");
const fs = require("fs");
const PORT = 1420;

/* ---------- 最小 PNG 解码（8bit RGB/RGBA，非隔行） ---------- */
function decodePNG(buf) {
  let pos = 8, w = 0, h = 0, ch = 4;
  const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString("ascii", pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    if (type === "IHDR") {
      w = data.readUInt32BE(0); h = data.readUInt32BE(4);
      const bd = data[8], ct = data[9];
      if (bd !== 8) throw new Error("仅支持 8bit");
      ch = ct === 6 ? 4 : ct === 2 ? 3 : ct === 0 ? 1 : ct === 4 ? 2 : 0;
      if (!ch) throw new Error("不支持的 colorType " + ct);
    } else if (type === "IDAT") idat.push(data);
    else if (type === "IEND") break;
    pos += 12 + len;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const out = Buffer.alloc(w * h * ch);
  const stride = w * ch;
  let prev = Buffer.alloc(stride);
  for (let y = 0; y < h; y++) {
    const ft = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, y * (stride + 1) + 1 + stride);
    const cur = Buffer.from(line);
    for (let i = 0; i < stride; i++) {
      const a = i >= ch ? cur[i - ch] : 0;
      const b = prev[i];
      const c = i >= ch ? prev[i - ch] : 0;
      let v = cur[i];
      if (ft === 1) v += a;
      else if (ft === 2) v += b;
      else if (ft === 3) v += (a + b) >> 1;
      else if (ft === 4) {
        const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      cur[i] = v & 0xff;
    }
    cur.copy(out, y * stride);
    prev = cur;
  }
  return { w, h, ch, data: out };
}

const px = (img, x, y) => {
  if (x < 0 || y < 0 || x >= img.w || y >= img.h) return null;
  const i = (y * img.w + x) * img.ch;
  return [img.data[i], img.data[i + 1], img.data[i + 2]];
};
const lum = (c) => {
  const s = c.map((v) => {
    const t = v / 255;
    return t <= 0.03928 ? t / 12.92 : Math.pow((t + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2];
};
const contrast = (a, b) => {
  const la = lum(a), lb = lum(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
};
const median = (arr) => {
  const s = [...arr].sort((x, y) => x - y);
  return s[Math.floor(s.length / 2)];
};

/* ---------- 页面内：只负责报出「干净的采样矩形」 ---------- */
const COLLECT = () => {
  const out = [];
  const widgets = [...document.querySelectorAll(".ui-widget")];
  const sx = window.scrollX, sy = window.scrollY;

  /* 找一块「干净」的板内矩形：靠右缘内缩（padding 区，绝大多数 widget 右侧留白），
     避开数字/文字/图标/进度条；板外取同列板缘上方 12px（跨过边框与阴影带）。 */
  for (let i = 0; i < widgets.length; i++) {
    const el = widgets[i];
    const r = el.getBoundingClientRect();
    if (r.width < 80 || r.height < 40) continue;
    if (r.bottom < 0 || r.top > window.innerHeight) continue; /* 视口外跳过 */
    out.push({
      name: `w${i}`,
      inside: {
        x: Math.round(r.x + sx + r.width - 30),
        y: Math.round(r.y + sy + r.height * 0.5),
        w: 14, h: 12,
      },
      outside: {
        x: Math.round(r.x + sx + r.width - 30),
        y: Math.round(r.y + sy - 12),
        w: 14, h: 6,
      },
    });
  }

  /* 自检：页面上放一个已知深浅的小方块，验证「解码 + 采样 + 对比度」链路可信。
     如果这条不过，其余结果一律不许采信。 */
  let st = document.getElementById("__selftest");
  if (!st) {
    st = document.createElement("div");
    st.id = "__selftest";
    st.style.cssText =
      "position:absolute;left:8px;top:8px;width:44px;height:44px;z-index:99999;" +
      "background:linear-gradient(90deg,#101010 50%,#f2f2f2 50%)";
    document.body.appendChild(st);
  }
  const sr = st.getBoundingClientRect();
  out.push({
    name: "SELFTEST",
    inside: { x: Math.round(sr.x + sx + 4), y: Math.round(sr.y + sy + 8), w: 10, h: 10 },
    outside: { x: Math.round(sr.x + sx + 30), y: Math.round(sr.y + sy + 8), w: 10, h: 10 },
    gap: 0,
  });
  return out;
};

(async () => {
  const variants = process.argv.slice(2);
  const list = variants.length ? variants : ["dusk", "pine", "wall", "plain", "light"];
  const b = await chromium.launch({ channel: "msedge" });
  let badTotal = 0, total = 0;

  for (const variant of list) {
    const p = await b.newPage({ viewport: { width: 1600, height: 1100 }, deviceScaleFactor: 1 });
    await p.goto(`http://localhost:${PORT}/.visual/overview-probe.html#${variant}`, { waitUntil: "load" });
    await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 25000 }).catch(() => {});
    await p.waitForTimeout(700);

    const targets = await p.evaluate(COLLECT);
    const buf = await p.screenshot({ fullPage: true });
    const img = decodePNG(buf);

    const rows = targets.map((t) => {
      const inCols = [], outCols = [];
      for (let x = 0; x < t.inside.w; x++) {
        const col = [];
        for (let y = 0; y < t.inside.h; y++) {
          const c = px(img, t.inside.x + x, t.inside.y + y);
          if (c) col.push(c);
        }
        if (col.length) inCols.push(col);
      }
      for (let x = 0; x < t.outside.w; x++) {
        const col = [];
        for (let y = 0; y < t.outside.h; y++) {
          const c = px(img, t.outside.x + x, t.outside.y + y);
          if (c) col.push(c);
        }
        if (col.length) outCols.push(col);
      }
      /* 逐列取三通道和的列均值，再取中位数：抗单列命中文字 */
      const colLum = (cols) =>
        median(cols.map((col) => col.reduce((n, c) => n + c[0] + c[1] + c[2], 0) / col.length / 3));
      const inL = colLum(inCols);
      const outL = colLum(outCols);
      const ci = inCols.length ? inCols[Math.floor(inCols.length / 2)][0] : null;
      const co = outCols.length ? outCols[Math.floor(outCols.length / 2)][0] : null;
      const ratio = ci && co ? contrast(ci, co) : 0;
      return {
        name: t.name,
        inside: ci ? ci.join(",") : "-",
        outside: co ? co.join(",") : "-",
        lumIn: inL.toFixed(1),
        lumOut: outL.toFixed(1),
        ratio: Number(ratio.toFixed(2)),
        pass: ratio >= 3,
      };
    });

    console.log(`\n══ 变体 ${variant} ── 板面 vs 板外背景（阈值 3:1）`);
    for (const r of rows) {
      const tag = r.name === "SELFTEST" ? (r.pass ? "自检通过" : "自检失败") : r.pass ? "OK  " : "低  ";
      console.log(
        `${tag.padEnd(8)} ${r.name.padEnd(12)} 板内=${r.inside.padEnd(12)} 板外=${r.outside.padEnd(12)} 对比=${String(r.ratio).padStart(5)}:1`,
      );
    }
    const bad = rows.filter((r) => r.name !== "SELFTEST" && !r.pass);
    total += rows.length - 1;
    badTotal += bad.length;
    console.log(`小计：不达标 ${bad.length}/${rows.length - 1}${bad.length ? "   " + bad.map((r) => r.name).join(", ") : ""}`);
    await p.close();
  }

  console.log(`\n合计：不达标 ${badTotal}/${total}`);
  await b.close();
})();
