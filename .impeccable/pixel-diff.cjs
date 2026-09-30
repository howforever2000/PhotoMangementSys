/**
 * PNG 像素级差异统计（Node 纯 JS 解码 PNG，无外部依赖）。
 * 用法：node .impeccable\pixel-diff.cjs a.png b.png [容差]
 * 输出：不同像素数 / 占比 / 最大通道差。
 */
const fs = require("fs");
const zlib = require("zlib");

function decodePng(file) {
  const buf = fs.readFileSync(file);
  if (buf.readUInt32BE(0) !== 0x89504e47) throw new Error("not png");
  let pos = 8;
  let width = 0;
  let height = 0;
  let bitDepth = 8;
  let colorType = 6;
  const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString("ascii", pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
      if (data[12] !== 0) throw new Error("interlaced png unsupported");
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") break;
    pos += 12 + len;
  }
  if (bitDepth !== 8) throw new Error("only 8-bit png");
  const ch = { 0: 1, 2: 3, 4: 2, 6: 4 }[colorType];
  if (!ch) throw new Error("unsupported colorType " + colorType);
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * ch;
  const out = Buffer.alloc(height * stride);
  let prev = Buffer.alloc(stride);
  let rp = 0;
  for (let y = 0; y < height; y += 1) {
    const filter = raw[rp++];
    const line = Buffer.from(raw.subarray(rp, rp + stride));
    rp += stride;
    for (let i = 0; i < stride; i += 1) {
      const a = i >= ch ? line[i - ch] : 0;
      const b = prev[i];
      const c = i >= ch ? prev[i - ch] : 0;
      let v = line[i];
      if (filter === 1) v += a;
      else if (filter === 2) v += b;
      else if (filter === 3) v += (a + b) >> 1;
      else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      line[i] = v & 0xff;
    }
    line.copy(out, y * stride);
    prev = line;
  }
  return { width, height, ch, data: out };
}

const [, , fa, fb, tolArg] = process.argv;
const tol = Number(tolArg ?? 2);
const A = decodePng(fa);
const B = decodePng(fb);
if (A.width !== B.width || A.height !== B.height) {
  console.log(`尺寸不同：${A.width}x${A.height} vs ${B.width}x${B.height}`);
  process.exit(2);
}
let diff = 0;
let maxd = 0;
const total = A.width * A.height;
let minX = A.width;
let minY = A.height;
let maxX = -1;
let maxY = -1;
for (let y = 0; y < A.height; y += 1) {
  for (let x = 0; x < A.width; x += 1) {
    const i = (y * A.width + x) * A.ch;
    let d = 0;
    for (let k = 0; k < 3; k += 1) d = Math.max(d, Math.abs(A.data[i + k] - B.data[i + k]));
    if (d > tol) {
      diff += 1;
      if (x < minX) minX = x;
      if (x > maxX) maxX = x;
      if (y < minY) minY = y;
      if (y > maxY) maxY = y;
    }
    if (d > maxd) maxd = d;
  }
}
console.log(`${fa} vs ${fb}`);
console.log(`不同像素 ${diff}/${total} = ${((diff / total) * 100).toFixed(3)}%  最大通道差 ${maxd}`);
if (diff > 0) console.log(`差异区域 bbox: x ${minX}~${maxX}, y ${minY}~${maxY}`);
process.exit(diff === 0 ? 0 : 1);
