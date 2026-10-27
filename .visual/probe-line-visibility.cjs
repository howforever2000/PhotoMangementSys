// 深色模式线条可见性测量：node .visual/probe-line-visibility.cjs [variant]
// 做法：页面内定位每条候选线的真实像素坐标 → 截图 → 自写最小 PNG 解码器取样 → 算 WCAG 对比度
// 判定阈值：非文字 UI 边界按 WCAG 1.4.11 需要 ≥3:1（相对承载面）
const { chromium } = require("playwright");
const zlib = require("zlib");
const PORT = process.env.VITE_PORT || "1420";
const variant = process.argv[2] || "wall";

/* ── 最小 PNG 解码器（8bit RGB/RGBA，非隔行） ── */
function decodePNG(buf) {
  let pos = 8;
  let w = 0, h = 0, colorType = 0, bitDepth = 0;
  const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString("ascii", pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    if (type === "IHDR") {
      w = data.readUInt32BE(0);
      h = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
    } else if (type === "IDAT") idat.push(data);
    else if (type === "IEND") break;
    pos += 12 + len;
  }
  if (bitDepth !== 8) throw new Error("bitDepth " + bitDepth + " 不支持");
  const channels = colorType === 6 ? 4 : colorType === 2 ? 3 : 0;
  if (!channels) throw new Error("colorType " + colorType + " 不支持");
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = w * channels;
  const out = Buffer.alloc(h * stride);
  let rp = 0;
  for (let y = 0; y < h; y++) {
    const filter = raw[rp++];
    const line = raw.subarray(rp, rp + stride);
    rp += stride;
    const cur = out.subarray(y * stride, (y + 1) * stride);
    const prev = y > 0 ? out.subarray((y - 1) * stride, y * stride) : null;
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? cur[x - channels] : 0;
      const b = prev ? prev[x] : 0;
      const c = prev && x >= channels ? prev[x - channels] : 0;
      let v = line[x];
      if (filter === 1) v += a;
      else if (filter === 2) v += b;
      else if (filter === 3) v += (a + b) >> 1;
      else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      cur[x] = v & 0xff;
    }
  }
  return { w, h, channels, data: out };
}
function px(img, x, y) {
  if (x < 0 || y < 0 || x >= img.w || y >= img.h) return null;
  const i = y * img.w * img.channels + x * img.channels;
  return [img.data[i], img.data[i + 1], img.data[i + 2]];
}
function srgb(c) { c /= 255; return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4); }
function lum(rgb) { return 0.2126 * srgb(rgb[0]) + 0.7152 * srgb(rgb[1]) + 0.0722 * srgb(rgb[2]); }
function contrast(a, b) {
  const l1 = lum(a), l2 = lum(b);
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
}
function avg(list) {
  const v = list.filter(Boolean);
  if (!v.length) return null;
  return [0, 1, 2].map((i) => v.reduce((s, p) => s + p[i], 0) / v.length);
}

/* ── 页面内：定位每条候选线的真实像素坐标 ── */
const COLLECT = () => {
  const out = [];
  const add = (name, sel, x, y, dx = 0, dy = 0) => out.push({ name, sel, x, y, dx, dy });

  const widgets = [...document.querySelectorAll(".ui-widget")];
  widgets.slice(0, 2).forEach((w, i) => {
    const r = w.getBoundingClientRect();
    const sx = r.x + window.scrollX, sy = r.y + window.scrollY;
    // 上边界：现在有「外侧暗环 + 内侧亮边」两层，各取一点
    add(`widget${i}-outer-ring`, ".ui-widget", Math.round(sx + 60), Math.round(sy - 1), 0, -3);
    add(`widget${i}-inner-line`, ".ui-widget", Math.round(sx + 60), Math.round(sy + 1), 0, 3);
    // 左边界
    add(`widget${i}-outer-ring-l`, ".ui-widget", Math.round(sx - 1), Math.round(sy + 50), -3, 0);
    add(`widget${i}-inner-line-l`, ".ui-widget", Math.round(sx + 1), Math.round(sy + 50), 3, 0);
  });

  const hero = document.querySelector(".ov-hero-r");
  if (hero) {
    const r = hero.getBoundingClientRect();
    add("hero-divider", ".ov-hero-r", Math.round(r.x + window.scrollX), Math.round(r.y + window.scrollY + 50), -4, 0);
  }

  /* 顶栏（AppHeader）：浮层玻璃在亮壁纸上是否读得出 */
  const mh = document.querySelector(".mh-inner");
  if (mh) {
    const r = mh.getBoundingClientRect();
    add("appheader-surface", ".mh-inner", Math.round(r.x + window.scrollX + 300), Math.round(r.y + window.scrollY + r.height / 2), 0, 0);
    add("appheader-vs-outside", ".mh-inner", Math.round(r.x + window.scrollX + 300), Math.round(r.y + window.scrollY + r.height / 2), 0, -14);
    add("appheader-top-edge", ".mh-inner", Math.round(r.x + window.scrollX + 300), Math.round(r.y + window.scrollY), 0, -6);
  }

  const pbar = document.querySelector(".w-pbar");
  if (pbar) {
    const r = pbar.getBoundingClientRect();
    const sx = r.x + window.scrollX, sy = r.y + window.scrollY;
    // 轨道：取填充段右端之外 2px（填充宽由 --w-pbar>i 决定，这里取最右 3px）
    const fillEl = pbar.querySelector("i");
    const fw = fillEl ? fillEl.getBoundingClientRect().width : 0;
    add("pbar-track", ".w-pbar", Math.round(sx + Math.min(r.width - 2, fw + 3)), Math.round(sy + r.height / 2), 0, 0);
    // 填充段（与板面比，证明强调色本身可见）
    add("pbar-fill", ".w-pbar>i", Math.round(sx + Math.min(fw - 4, 20)), Math.round(sy + r.height / 2), 0, 0);
  }

  const rankRow = document.querySelector(".w-rank-row");
  if (rankRow) {
    const track = rankRow.querySelector("i");
    const fill = rankRow.querySelector("i b");
    if (track) {
      const r = track.getBoundingClientRect();
      const fw = fill ? fill.getBoundingClientRect().width : 0;
      add("rank-track", ".w-rank-row i", Math.round(r.x + window.scrollX + Math.min(r.width - 2, fw + 3)), Math.round(r.y + window.scrollY + r.height / 2), 0, 0);
    }
    if (fill) {
      const r = fill.getBoundingClientRect();
      add("rank-fill", ".w-rank-row i b", Math.round(r.x + window.scrollX + Math.min(r.width - 4, 10)), Math.round(r.y + window.scrollY + r.height / 2), 0, 0);
    }
  }

  const bars = [...document.querySelectorAll(".w-bars i")];
  const trackBar = bars.find((el) => !el.classList.contains("on") && !el.classList.contains("is-zero"));
  const onBar = bars.find((el) => el.classList.contains("on"));
  if (trackBar) {
    const r = trackBar.getBoundingClientRect();
    add("bars-track", ".w-bars i", Math.round(r.x + window.scrollX + r.width / 2), Math.round(r.y + window.scrollY + r.height / 2), 0, 0);
  }
  if (onBar) {
    const r = onBar.getBoundingClientRect();
    add("bars-highlight", ".w-bars i.on", Math.round(r.x + window.scrollX + r.width / 2), Math.round(r.y + window.scrollY + r.height / 2), 0, 0);
  }

  const geo = document.querySelector(".w-geo");
  if (geo) {
    const r = geo.getBoundingClientRect();
    add("geo-dotfield", ".w-geo", Math.round(r.x + window.scrollX + 10), Math.round(r.y + window.scrollY + 10), 0, 0);
  }

  const cover = document.querySelector(".w-cv");
  if (cover) {
    const r = cover.getBoundingClientRect();
    const sx = r.x + window.scrollX, sy = r.y + window.scrollY;
    // 封面内描边（1px inset）在封面内部顶边
    add("cover-inset", ".w-cv", Math.round(sx + 30), Math.round(sy + 1), 0, 0);
    // 对比对象 = 封面图中心（图片本身）
    add("cover-fill", ".w-cv img", Math.round(sx + r.width / 2), Math.round(sy + r.height / 2), 0, 0);
  }

  /* ── 取样自检：取“已知高对比参照”的 9x9 盒内极值差。
     参照 = 面板内的金色文字（--w-accent），它与板面的对比必然很高。
     若这个差值也很小，说明取样/解码环节坏了，后续数据不可信。 */
  const gold = [...document.querySelectorAll(".w-stat-unit, .ui-widget-link, .w-rank-row em, .w-cv-name")]
    .find((el) => el.textContent && el.textContent.trim().length > 0);
  if (gold) {
    const r = gold.getBoundingClientRect();
    out.push({
      name: "SELFTEST-gold-box",
      sel: ".gold",
      box: { x: Math.round(r.x + window.scrollX), y: Math.round(r.y + window.scrollY), w: Math.max(4, Math.round(r.width)), h: Math.max(4, Math.round(r.height)) },
      selfTest: true,
    });
  }
  /* 板面中心（作为板内基准色） */
  const w0 = widgets[0];
  if (w0) {
    const r = w0.getBoundingClientRect();
    /* 板面 vs 板外背景：玻璃板真正靠这个差读出“这里有一块”。
       取板底边内侧 2px 与板外下方 8px（间距 14px，确保落在板外）。 */
    out.push({
      name: "widget0-surface",
      sel: ".ui-widget",
      x: Math.round(r.x + window.scrollX + r.width / 2),
      y: Math.round(r.y + window.scrollY + r.height - 3),
      dx: 0,
      dy: 0,
    });
    out.push({
      name: "widget0-surface-vs-outside",
      sel: ".ui-widget",
      x: Math.round(r.x + window.scrollX + r.width / 2),
      y: Math.round(r.y + window.scrollY + r.height - 3),
      dx: 0,
      dy: 10,
    });
    /* 接触阴影：板外 3px（影内）vs 板外 13px（影外）—— 亮壁纸上这是唯一有效的边界信号 */
    out.push({
      name: "widget0-contact-shadow",
      sel: ".ui-widget",
      x: Math.round(r.x + window.scrollX + r.width / 2),
      y: Math.round(r.y + window.scrollY + r.height + 3),
      dx: 0,
      dy: 10,
    });
  }
  return out;
};

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 1 });
  p.on("pageerror", (e) => console.log("错误", String(e).split("\n")[0]));
  await p.goto(`http://localhost:${PORT}/.visual/overview-probe.html#${variant}`, { waitUntil: "load" });
  await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 25000 }).catch(() => {});
  await p.waitForTimeout(600);

  const probes = await p.evaluate(COLLECT);
  const buf = await p.screenshot({ fullPage: true });
  const img = decodePNG(buf);

  const rows = probes.map((pr) => {
    if (pr.selfTest) {
      /* 盒内极值差 = 解码链路自检 */
      let mn = [255, 255, 255], mx = [0, 0, 0];
      for (let y = pr.box.y; y < pr.box.y + pr.box.h; y++) {
        for (let x = pr.box.x; x < pr.box.x + pr.box.w; x++) {
          const c = px(img, x, y);
          if (!c) continue;
          for (let i = 0; i < 3; i++) { mn[i] = Math.min(mn[i], c[i]); mx[i] = Math.max(mx[i], c[i]); }
        }
      }
      const c = contrast(mn, mx);
      return { name: pr.name, line: mn.join(","), ref: mx.join(","), ratio: Number(c.toFixed(2)), pass: c >= 3 };
    }
    const line = avg([px(img, pr.x, pr.y), px(img, pr.x + 1, pr.y), px(img, pr.x, pr.y + 1)]);
    const ref = avg([px(img, pr.x + pr.dx, pr.y + pr.dy), px(img, pr.x + pr.dx + 1, pr.y + pr.dy)]);
    const c = line && ref ? contrast(line, ref) : 0;
    return {
      name: pr.name,
      line: line ? line.map((v) => Math.round(v)).join(",") : "-",
      ref: ref ? ref.map((v) => Math.round(v)).join(",") : "-",
      ratio: Number(c.toFixed(2)),
      pass: c >= 3,
    };
  });

  console.log(`\n变体 ${variant} —— 线条可见性（阈值 3:1，WCAG 1.4.11 非文字边界）`);
  for (const r of rows) {
    console.log(
      `${r.pass ? "OK  " : "低  "} ${r.name.padEnd(22)} 线=${String(r.line).padEnd(12)} 承载=${String(r.ref).padEnd(12)} 对比=${r.ratio}:1`,
    );
  }
  const bad = rows.filter((r) => !r.pass);
  console.log(`\n不达标 ${bad.length}/${rows.length}`);
  await b.close();
})();
