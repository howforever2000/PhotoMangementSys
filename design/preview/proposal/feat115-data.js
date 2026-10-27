/* 效果图共用的假封面生成（与 index.html 的 mock 同一套伪随机算法，
   保证同一 seed 每次刷新得到同一张"照片"） */
function photo(seed, w = 320, h = 200) {
  let x = seed * 9301 + 49297;
  const rnd = () => ((x = (x * 9301 + 49297) % 233280) / 233280);
  const h1 = Math.floor(rnd() * 360);
  const h2 = (h1 + 40 + Math.floor(rnd() * 80)) % 360;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}">
    <defs><radialGradient id="g" cx="${30 + rnd() * 40}%" cy="${20 + rnd() * 30}%" r="90%">
      <stop offset="0%" stop-color="hsl(${h1},55%,70%)"/>
      <stop offset="55%" stop-color="hsl(${h2},42%,42%)"/>
      <stop offset="100%" stop-color="hsl(${h2},38%,17%)"/>
    </radialGradient></defs>
    <rect width="${w}" height="${h}" fill="url(#g)"/>
    <circle cx="${w * (0.2 + rnd() * 0.6)}" cy="${h * (0.15 + rnd() * 0.4)}" r="${h * 0.18}" fill="hsl(${h1},70%,80%)" opacity=".65"/>
  </svg>`;
  return "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
}

/* 相册数据（与 mock 一致的 9 个相册） */
const ALBUMS = [
  { id: 1, name: "2026 毕业季", n: 108, date: "2026-06", scanned: true, c: 1 },
  { id: 2, name: "家人", n: 84, date: "2025-10", scanned: true, c: 2 },
  { id: 3, name: "猫咪日常", n: 36, date: "2026-03", scanned: true, c: 3 },
  { id: 4, name: "秋日街头", n: 62, date: "2026-09", scanned: true, c: 4 },
  { id: 5, name: "落日山峦", n: 28, date: "2026-05", scanned: false, c: 5 },
  { id: 6, name: "海边", n: 41, date: "2025-07", scanned: true, c: 6 },
  { id: 7, name: "夜景拾遗", n: 23, date: "2024-11", scanned: false, c: 7 },
  { id: 8, name: "文档扫描", n: 12, date: "2024-09", scanned: true, c: 8 },
  { id: 90, name: "归档", n: 5, date: "", scanned: false, c: 9 },
];

/* 时间线行 */
const ROWS = [];
(function () {
  const cities = ["北京市", "上海市", "成都市", "杭州市", null];
  const cats = ["人物", "风景", "夜景", "文档", null];
  let id = 1;
  for (const [year, months] of [[2026, [3, 5, 6, 9]], [2025, [7, 10, 12]], [2024, [9, 11]]]) {
    for (const m of months) {
      const n = 6 + ((id * 7) % 10);
      for (let i = 0; i < n; i++) {
        ROWS.push({
          id: id,
          seed: id,
          year, month: m,
          label: id % 4 === 0 ? null : cats[id % cats.length],
          location: id % 5 === 0 ? null : cities[id % cities.length],
        });
        id++;
      }
    }
  }
})();

const PERSONS = [
  { id: "P01", name: "爸爸", n: 96, c: 2 },
  { id: "P02", name: "妈妈", n: 88, c: 4 },
  { id: "P03", name: "小舟", n: 64, c: 1 },
  { id: "P04", name: "同学A", n: 31, c: 6 },
  { id: "P05", name: null, n: 27, c: 3 },
  { id: "P06", name: null, n: 19, c: 8 },
  { id: "P07", name: null, n: 14, c: 5 },
];

const CITIES = [
  { name: "北京", n: 186 }, { name: "上海", n: 142 }, { name: "成都", n: 96 },
  { name: "杭州", n: 61 }, { name: "三亚", n: 41 }, { name: "西安", n: 24 },
];

const CATS = [
  { id: "person", icon: "🧑", name: "人物", n: 220, words: 12, c: 1, wide: false },
  { id: "scene", icon: "🏞️", name: "风景", n: 168, words: 9, c: 5, wide: true },
  { id: "night", icon: "🌃", name: "夜景", n: 96, words: 7, c: 7, wide: false },
  { id: "doc", icon: "📄", name: "文档", n: 44, words: 5, c: 8, wide: false },
  { id: "food", icon: "🍜", name: "美食", n: 38, words: 6, c: 2, wide: false },
  { id: "cat", icon: "🐱", name: "猫", n: 31, words: 4, c: 3, wide: false },
];

/* 主题：由 body[data-theme] 决定，不需要 JS 参与 */
function setTheme(t) { document.body.dataset.theme = t; }
