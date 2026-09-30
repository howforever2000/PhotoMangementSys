/**
 * 材质对比像素验收：不依赖读图工具，直接算像素。
 *  1. 证据角标校验：截图 (10,10)-(320,44) 区域应有绿字（#0f0 系）像素；
 *  2. 磨砂 vs 液态差异度：两图逐像素比较，差异像素占比 + 平均色差；
 *  3. 卡片区域（module-card 第一张，约 x245-710 y175-290）单独算差异。
 */
const fs = require("fs");
const path = require("path");
const zlib = require("zlib");

/** 极简 PNG 解码（8bit RGBA/RGB，无隔行）——只为我们自己的截图服务 */
function decodePNG(buf) {
  if (buf.readUInt32BE(0) !== 0x89504e47) throw new Error("not png");
  let off = 8;
  let w = 0, h = 0, bitDepth = 0, colorType = 0;
  const idat = [];
  while (off < buf.length) {
    const len = buf.readUInt32BE(off);
    const type = buf.toString("ascii", off + 4, off + 8);
    const data = buf.subarray(off + 8, off + 8 + len);
    if (type === "IHDR") {
      w = data.readUInt32BE(0);
      h = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
      if (data[12] !== 0) throw new Error("interlaced not supported");
      if (bitDepth !== 8 || (colorType !== 6 && colorType !== 2)) throw new Error(`unsupported png ct=${colorType} bd=${bitDepth}`);
    } else if (type === "IDAT") idat.push(data);
    else if (type === "IEND") break;
    off += 12 + len;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const ch = colorType === 6 ? 4 : 3;
  const stride = w * ch;
  const out = Buffer.alloc(h * stride);
  let pos = 0;
  for (let y = 0; y < h; y++) {
    const filter = raw[pos++];
    const row = raw.subarray(pos, pos + stride);
    pos += stride;
    const cur = out.subarray(y * stride, (y + 1) * stride);
    const prev = y > 0 ? out.subarray((y - 1) * stride, y * stride) : null;
    for (let i = 0; i < stride; i++) {
      const a = i >= ch ? cur[i - ch] : 0;
      const b = prev ? prev[i] : 0;
      const c = prev && i >= ch ? prev[i - ch] : 0;
      let v = row[i];
      switch (filter) {
        case 1: v = (v + a) & 255; break;
        case 2: v = (v + b) & 255; break;
        case 3: v = (v + ((a + b) >> 1)) & 255; break;
        case 4: {
          const p = a + b - c;
          const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
          v = (v + (pa <= pb && pa <= pc ? a : pb <= pc ? b : c)) & 255;
          break;
        }
      }
      cur[i] = v;
    }
  }
  return { w, h, ch, data: out };
}

function px(img, x, y) {
  const i = (y * img.w + x) * img.ch;
  return [img.data[i], img.data[i + 1], img.data[i + 2]];
}

function countGreenProof(img, x0, y0, x1, y1) {
  let n = 0;
  for (let y = y0; y < y1; y++)
    for (let x = x0; x < x1; x++) {
      const [r, g, b] = px(img, x, y);
      if (g > 180 && r < 120 && b < 120) n++;
    }
  return n;
}

function diffStats(a, b, x0, y0, x1, y1) {
  let diff = 0, total = 0, sum = 0;
  for (let y = y0; y < y1; y++)
    for (let x = x0; x < x1; x++) {
      const pa = px(a, x, y), pb = px(b, x, y);
      const d = Math.abs(pa[0] - pb[0]) + Math.abs(pa[1] - pb[1]) + Math.abs(pa[2] - pb[2]);
      if (d > 24) diff++;
      sum += d;
      total++;
    }
  return { pct: (diff / total) * 100, avg: sum / total };
}

const DIR = path.join(__dirname, "review");
const fro = decodePNG(fs.readFileSync(path.join(DIR, "material-frosted.png")));
const liq = decodePNG(fs.readFileSync(path.join(DIR, "material-liquid.png")));

let fail = 0;
const assert = (cond, msg) => {
  console.log((cond ? "PASS" : "FAIL") + ": " + msg);
  if (!cond) fail++;
};

// 1. 证据角标（截图前注入的绿字 DOM）
assert(countGreenProof(fro, 8, 8, 340, 46) > 50, `磨砂图含绿字角标: ${countGreenProof(fro, 8, 8, 340, 46)} px`);
assert(countGreenProof(liq, 8, 8, 340, 46) > 50, `液态图含绿字角标: ${countGreenProof(liq, 8, 8, 340, 46)} px`);

// 2. 角标首段是材质名（frosted/liquid）：只比对标签头部 x 8-115
const proof = diffStats(fro, liq, 8, 8, 115, 46);
assert(proof.pct > 3, `两图角标材质名不同（各自标注）: 差异 ${proof.pct.toFixed(1)}%`);

// 3. 全图差异度：材质切换必须肉眼可辨的量化下限
const full = diffStats(fro, liq, 0, 0, fro.w, fro.h);
assert(full.pct > 5, `全图差异像素占比 ${full.pct.toFixed(1)}% > 5%（材质可辨）`);
console.log(`   全图平均色差 ${full.avg.toFixed(2)} / 765`);

// 4. 卡片区域（module-card 左上第一张）差异应显著（材质主战场）
const card = diffStats(fro, liq, 250, 180, 700, 285);
assert(card.pct > 8, `卡片区域差异占比 ${card.pct.toFixed(1)}% > 8%`);
console.log(`   卡片区平均色差 ${card.avg.toFixed(2)}`);

// 5. 两图尺寸一致（公平比较前提）
assert(fro.w === liq.w && fro.h === liq.h, `尺寸一致 ${fro.w}x${fro.h}`);

process.exit(fail ? 1 : 0);
