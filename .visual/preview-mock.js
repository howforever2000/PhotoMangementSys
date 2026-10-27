/**
 * 浏览器预览 Mock（仅 `.visual/preview.html` 使用，不进业务代码）
 *
 * 用法：Vite 启动后访问 /.visual/preview.html#/albums 等。
 * 它在业务模块加载**之前**注册 window.__TAURI_INTERNALS__，
 * 让 @tauri-apps/api/core 的 invoke 走到我们的桩函数而不是真后端。
 *
 * 为什么要在这里做而不是起 tauri dev：无头环境跑不起 WebView2；
 * 而 Playwright 截图需要真实页面（探针页绕过了页面自身的加载逻辑，
 * 会漏掉"数据为空时页面长什么样"这类真实问题）。
 */

/* ---------- 路由改写：preview.html 不在根路径 ----------
 * 应用用 createWebHistory（路径式路由），直接访问
 * /.visual/preview.html#/albums 时 router 看到的是 pathname="/.visual/preview.html"，
 * 匹配不到任何页面 —— 表现为「整页只剩背景渐变」（实测踩过）。
 * 在应用启动前把地址栏改写成目标路径，document 仍是带 mock 的这一份。
 * 注意：改写后刷新会回到真实 index.html（那份没有 mock），刷新即失效。
 *
 * FEAT-122：这段原先只存在于 preview.html 的**内联副本**里，导致改 preview-mock.js
 * 却不生效（两份副本漂移）。现在 mock 只有本文件一份，preview.html 用外链引入。
 */
{
  const hashPath = location.hash.replace(/^#/, "");
  const target = hashPath.startsWith("/") ? hashPath : "/albums";
  history.replaceState(null, "", target + location.search);
}

/* ---------- 工具：造一张"照片感"的 SVG 占位图（转 data URL） ---------- */
function photo(seed, w = 320, h = 200) {
  // 用 seed 生成稳定的伪随机色相，保证每次刷新同一相册封面不变
  let x = seed * 9301 + 49297;
  const rnd = () => ((x = (x * 9301 + 49297) % 233280) / 233280);
  const h1 = Math.floor(rnd() * 360);
  const h2 = (h1 + 40 + Math.floor(rnd() * 80)) % 360;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}">
    <defs><radialGradient id="g" cx="${30 + rnd() * 40}%" cy="${20 + rnd() * 30}%" r="90%">
      <stop offset="0%" stop-color="hsl(${h1},60%,72%)"/>
      <stop offset="55%" stop-color="hsl(${h2},45%,42%)"/>
      <stop offset="100%" stop-color="hsl(${h2},40%,18%)"/>
    </radialGradient></defs>
    <rect width="${w}" height="${h}" fill="url(#g)"/>
    <circle cx="${w * (0.2 + rnd() * 0.6)}" cy="${h * (0.15 + rnd() * 0.4)}" r="${h * 0.18}" fill="hsl(${h1},70%,80%)" opacity=".7"/>
  </svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

/** 缩略图映射：path → data URL（真实应用用 get_photo_thumbs 返回本地缓存路径，
 *  前端再 convertFileSrc；mock 直接给 data URL，view 里 fileUrl() 会原样放行） */
const THUMBS = new Map();
let thumbSeq = 0;
function thumbFor(path) {
  if (!THUMBS.has(path)) THUMBS.set(path, photo(++thumbSeq, 256, 160));
  return THUMBS.get(path);
}

/* ---------- 数据 ---------- */
const ALBUMS = [
  { id: 1, name: "2026 毕业季", path: "C:/photos/2026毕业季", description: "六月拍的那批", cover_path: "c:1", created_at: 0, updated_at: 0, photo_count: 108, shoot_time: "2026-06-15", size_bytes: 108 * 4.2e6, scanned_photo_count: 108, location: "北京市", tags: ["毕业"], folder_id: null, folder_path: "", kind: "normal", merged_sources: [] },
  { id: 2, name: "家人", path: "C:/photos/家人", description: null, cover_path: "c:2", created_at: 0, updated_at: 0, photo_count: 84, shoot_time: "2025-10-02", size_bytes: 84 * 3.8e6, scanned_photo_count: 84, location: "成都市", tags: [], folder_id: null, folder_path: "", kind: "normal", merged_sources: [] },
  { id: 3, name: "猫咪日常", path: "C:/photos/猫咪", description: "橘子", cover_path: "c:3", created_at: 0, updated_at: 0, photo_count: 36, shoot_time: "2026-03-20", size_bytes: 36 * 3.1e6, scanned_photo_count: 36, location: "上海市", tags: [], folder_id: null, folder_path: "", kind: "normal", merged_sources: [] },
  { id: 4, name: "秋日街头", path: "C:/photos/街头", description: null, cover_path: "c:4", created_at: 0, updated_at: 0, photo_count: 62, shoot_time: "2026-09-08", size_bytes: 62 * 4.0e6, scanned_photo_count: 62, location: "杭州市", tags: [], folder_id: null, folder_path: "", kind: "normal", merged_sources: [] },
  { id: 5, name: "落日山峦", path: "C:/photos/山峦", description: null, cover_path: "c:5", created_at: 0, updated_at: 0, photo_count: 28, shoot_time: "2026-05-01", size_bytes: 28 * 5.5e6, scanned_photo_count: 0, location: null, tags: [], folder_id: null, folder_path: "", kind: "normal", merged_sources: [] },
  { id: 6, name: "海边", path: "C:/photos/海边", description: null, cover_path: "c:6", created_at: 0, updated_at: 0, photo_count: 41, shoot_time: "2025-07-20", size_bytes: 41 * 4.4e6, scanned_photo_count: 41, location: "三亚市", tags: [], folder_id: null, folder_path: "", kind: "normal", merged_sources: [] },
  { id: 7, name: "夜景拾遗", path: "C:/photos/夜景", description: null, cover_path: "c:7", created_at: 0, updated_at: 0, photo_count: 23, shoot_time: "2024-11-11", size_bytes: 23 * 6.2e6, scanned_photo_count: 0, location: null, tags: [], folder_id: null, folder_path: "", kind: "normal", merged_sources: [] },
  { id: 8, name: "文档扫描", path: "C:/photos/文档", description: null, cover_path: null, created_at: 0, updated_at: 0, photo_count: 12, shoot_time: "2024-09-01", size_bytes: 12 * 0.8e6, scanned_photo_count: 12, location: null, tags: [], folder_id: null, folder_path: "", kind: "normal", merged_sources: [] },
  { id: 90, name: "归档", path: "", description: null, cover_path: null, created_at: 0, updated_at: 0, photo_count: 5, shoot_time: null, size_bytes: 0, scanned_photo_count: 0, location: null, tags: [], folder_id: null, folder_path: "", kind: "archived", merged_sources: [] },
];

/** 时间线行（与 ContentSearchHit 对齐） */
function makeRows() {
  const rows = [];
  const cities = ["北京市", "上海市", "成都市", "杭州市", null];
  const cats = ["人物", "风景", "夜景", "文档", null];
  let id = 1;
  for (const [year, months] of [[2026, [3, 5, 6, 9]], [2025, [7, 10, 12]], [2024, [9, 11]]]) {
    for (const m of months) {
      const n = 6 + ((id * 7) % 10);
      for (let i = 0; i < n; i++) {
        const album = ALBUMS[(id * 3) % 6];
        const shoot = `${year}-${String(m).padStart(2, "0")}-${String(1 + (i % 27)).padStart(2, "0")}`;
        rows.push({
          id: id++,
          path: `C:/photos/${year}${m}_${i}.jpg`,
          album_id: album.id,
          album_name: album.name,
          shoot_time: id % 17 === 0 ? null : shoot,
          location: id % 5 === 0 ? null : cities[id % cities.length],
          label: id % 4 === 0 ? null : cats[id % cats.length],
          category: id % 4 === 0 ? "other" : cats[id % cats.length] ?? "other",
          confidence: 0.4 + ((id * 13) % 60) / 100,
          person_ids: id % 6 === 0 ? ["P01"] : [],
          iso: 200, aperture: "f/2.8", focal_length: "35mm", lat: null, lon: null,
        });
      }
    }
  }
  return rows;
}
const ROWS = makeRows();

/* 人物（字段与真实 `PersonInfo` 对齐：id / name / face_count / created_at）。
   旧版这里是 person_id / photo_count —— 跟后端不符，导致探针里卡片上的编号与张数渲染成空。 */
const PERSONS = Array.from({ length: 14 }, (_, i) => ({
  id: `P${String(i + 1).padStart(2, "0")}`,
  name: i < 4 ? ["爸爸", "妈妈", "小舟", "同学A"][i] : null,
  face_count: 3 + ((i * 37) % 90),
  created_at: "2026-0" + ((i % 9) + 1) + "-1" + (i % 9),
}));

const LOCATIONS = [
  { location: "北京市", count: 186, lat: 39.90, lon: 116.40, cover_path: "c:1", cover_album_id: 1 },
  { location: "上海市", count: 142, lat: 31.23, lon: 121.47, cover_path: "c:3", cover_album_id: 3 },
  { location: "成都市", count: 96, lat: 30.57, lon: 104.07, cover_path: "c:2", cover_album_id: 2 },
  { location: "杭州市", count: 61, lat: 30.27, lon: 120.15, cover_path: "c:4", cover_album_id: 4 },
  { location: "三亚市", count: 41, lat: 18.25, lon: 109.51, cover_path: "c:6", cover_album_id: 6 },
  { location: "西安市", count: 24, lat: 34.34, lon: 108.94, cover_path: null, cover_album_id: null },
  { location: null, count: 31, lat: null, lon: null, cover_path: null, cover_album_id: null },
];

const CATEGORIES = [
  { category: "人物", count: 220, cover_path: "c:1" },
  { category: "风景", count: 168, cover_path: "c:5" },
  { category: "夜景", count: 96, cover_path: "c:7" },
  { category: "文档", count: 44, cover_path: null },
];

/* ---------- FEAT-122：时间线的按月分页桩 ----------
 * 这里单独造一份**大库**（约 2.5 万行），因为要验证的正是「DOM 不随库变大而膨胀」
 * 与「跳转落点精确」两件事 —— 用原来那几百行 mock 测不出来。 */
const TL_ROWS = (() => {
  const rows = [];
  let id = 1;
  const cities = ["北京市", "上海市", "成都市", "杭州市", "西安市", "三亚市"];
  const cats = ["人像", "风景", "夜景", "美食", "猫咪", "建筑"];
  const push = (y, m, n) => {
    for (let i = 0; i < n; i++) {
      const album = ALBUMS[id % ALBUMS.length];
      const shoot = y
        ? `${y}-${String(m).padStart(2, "0")}-${String(1 + (i % 27)).padStart(2, "0")}`
        : null;
      rows.push({
        id: id,
        path: `C:/photos/${y ?? "x"}${m ?? 0}/${id}.jpg`,
        album_id: album.id,
        album_name: album.name,
        shoot_time: shoot,
        location: id % 5 === 0 ? null : cities[id % cities.length],
        label: id % 4 === 0 ? null : cats[id % cats.length],
      });
      id += 1;
    }
  };
  for (let y = 2008; y <= 2026; y++) {
    for (let m = 1; m <= 12; m++) {
      // 大小月分明：既要有 10 张的小月，也要有 900+ 的大月
      push(y, m, 8 + ((y * 13 + m * 37) % 120));
    }
  }
  push(null, null, 4840); // 「未记录拍摄时间」那一团（实测本机就是 4,840 张）
  return rows;
})();

const TL_INDEX = (() => {
  const map = new Map();
  let noTime = 0;
  let noLoc = 0;
  for (const r of TL_ROWS) {
    const ym = r.shoot_time ? r.shoot_time.slice(0, 7) : "";
    map.set(ym, (map.get(ym) ?? 0) + 1);
    if (!r.shoot_time) noTime += 1;
    if (!r.location) noLoc += 1;
  }
  const months = [...map.entries()]
    .sort((a, b) => (a[0] === "" ? 1 : b[0] === "" ? -1 : a[0].localeCompare(b[0])))
    .map(([ym, count]) => ({
      year: ym ? ym.slice(0, 4) : "未记录",
      month: ym ? ym.slice(5, 7) : "",
      count,
    }));
  return { months, total: TL_ROWS.length, no_time: noTime, no_location: noLoc };
})();

/** 请求计数（探针要断言「进页面只发了个位数请求」） */
window.__PM_MOCK_CALLS__ = {};

/* ---------- invoke 桩 ---------- */
const HANDLERS = {
  get_current_user: () => ({ id: 1, username: "yuanhao", email: "y@local", phone: "", created_at: 0, avatar: null }),
  get_albums: () => ALBUMS,
  get_manual_tree: () => [],
  list_timeline: (args) => {
    // 按 user_id 过滤（mock 不区分用户）
    return ROWS;
  },
  /* FEAT-122：时间线骨架 + 按月分页 */
  timeline_index: () => TL_INDEX,
  timeline_month: (args) => {
    const { year, month } = args ?? {};
    const limit = Number(args?.limit ?? 0);
    const offset = Number(args?.offset ?? 0);
    const all = TL_ROWS.filter((r) => {
      if (year === "未记录" || !year) return !r.shoot_time;
      if (!r.shoot_time) return false;
      if (r.shoot_time.slice(0, 4) !== year) return false;
      return month ? r.shoot_time.slice(5, 7) === month : r.shoot_time.length < 7;
    });
    const page = limit > 0 ? all.slice(offset, offset + limit) : all.slice(offset);
    return page.map((r) => ({ ...r, thumb_path: `c:${r.path}` }));
  },
  /* FEAT-123：概览快照（从 TL_ROWS 派生，字段与 Rust 侧 OverviewStats 对齐） */
  overview_stats: () => {
    const months = new Map();
    let noTime = 0;
    let withFace = 0;
    let withGps = 0;
    let yMin = null;
    let yMax = null;
    for (const r of TL_ROWS) {
      if (!r.shoot_time) noTime += 1;
      else {
        const ym = r.shoot_time.slice(0, 7);
        months.set(ym, (months.get(ym) ?? 0) + 1);
        const y = Number(r.shoot_time.slice(0, 4));
        yMin = yMin === null ? y : Math.min(yMin, y);
        yMax = yMax === null ? y : Math.max(yMax, y);
      }
      if (r.id % 6 === 0) withFace += 1;
      if (r.id % 3 === 0) withGps += 1;
    }
    const card = (r) => ({
      path: r.path,
      album_id: r.album_id,
      album_name: r.album_name,
      shoot_time: r.shoot_time,
      location: r.location,
    });
    return {
      total_scanned: TL_ROWS.length,
      no_time: noTime,
      with_face: withFace,
      with_gps: withGps,
      year_min: yMin,
      year_max: yMax,
      months: [...months.entries()].sort((a, b) => a[0].localeCompare(b[0])),
      recent: TL_ROWS.slice(-6).reverse().map(card),
      on_this_day: null,
      on_this_day_count: 0,
      indexed: TL_ROWS.length - 9,
      stale: 9,
      known: TL_ROWS.length + 6,
      semantic_categories: 4,
      model: "mock-clip",
      signature: "mock",
      built_at: Date.now(),
    };
  },
  get_photo_thumbs: (args) => (args.paths ?? []).map((p) => [p, thumbFor(p)]),
  /* FEAT-124：人物卡封面（4:5）—— mock 直接给彩色 data URL */
  get_person_covers_bulk: (args) =>
    (args.pids ?? []).map((pid) => thumbFor(`cover:${pid}`)),
  /* 圆形小头像（合并弹窗用）：mock 一律给图，缺了会让 PersonGallery 在 load() 里抛错 */
  get_person_avatars_bulk: (args) =>
    (args.pids ?? []).map((pid) => thumbFor(`avatar:${pid}`)),
  get_person_cover: (args) => thumbFor(`cover:${args.pid ?? "x"}`),
  list_persons: () => PERSONS,
  list_photo_locations: () => LOCATIONS,
  list_photos_by_location: (args) => ROWS.filter((r) => r.location === args.location).slice(0, 24),
  list_photos_by_category: (args) => ROWS.filter((r) => r.category === args.category).slice(0, 24),
  list_categories: () => CATEGORIES,
  category_index_stats: () => ({ total: ROWS.length, indexed: ROWS.length, categories: CATEGORIES.length }),
  semantic_status: () => ({ loaded: true, model: "mock-clip", dim: 512, ready: true, warming: false }),
  smart_search: (args) => ({ hits: ROWS.slice(0, 18), total: ROWS.length }),
  search_albums: (args) => {
    const kw = (args.keyword ?? "").toLowerCase();
    return { albums: ALBUMS.filter((a) => a.name.toLowerCase().includes(kw)) };
  },
  convert_file_src: (p) => p,
  open_folder: () => {},
  scan_album: () => ({ total: 0, scanned: 0, skipped: 0 }),
};

/** convertFileSrc 在 mock 下直接放行（data URL 不能再包一层） */
window.__TAURI_INTERNALS__ = {
  transformCallback(cb) { return cb; },
  /** @tauri-apps/api 2.x 的 convertFileSrc 走这里（不是独立函数） */
  convertFileSrc(filePath, protocol) {
    if (filePath && filePath.startsWith("data:")) return filePath;
    if (filePath && filePath.startsWith("c:")) return thumbFor(filePath);
    return filePath ?? "";
  },
  /** 插件 API（dialog 等）在页面里也可能被调用；给个空实现避免硬崩 */
  isTauri: true,
  invoke(cmd, args, opts) {
    const calls = window.__PM_MOCK_CALLS__;
    calls[cmd] = (calls[cmd] ?? 0) + 1;
    const h = HANDLERS[cmd];
    if (!h) {
      console.warn(`[mock] 未实现的 invoke: ${cmd}`, args ?? "");
      return Promise.resolve(null);
    }
    return Promise.resolve(h(args ?? {}));
  },
};

// convertFileSrc 的真实实现会拼 asset:// 协议；mock 环境直接透传
window.__PM_MOCK_CONVERT__ = (p) => p;

/* ---------- 材质 / 透明度 / 明暗 探针钩子（?mat=liquid&alpha=0.3&mode=light） ----------
 * 原先是内联在 preview.html 里的一段；FEAT-122 把整份 mock 收敛到本文件
 * （两处副本导致「改了 preview-mock.js 却没生效」），这里一并搬过来。
 */
{
  const q = new URLSearchParams(location.search);
  const wanted = { mat: q.get("mat"), mode: q.get("mode"), alpha: q.get("alpha") };
  if (wanted.mat || wanted.mode || wanted.alpha) {
    const wait = setInterval(async () => {
      if (!document.body.className.includes("mat-")) return;
      clearInterval(wait);
      const { useThemeStore } = await import("/src/stores/theme");
      const theme = useThemeStore();
      if (wanted.mode === "light" || wanted.mode === "dark") theme.mode = wanted.mode;
      if (wanted.mat === "liquid" || wanted.mat === "plain") theme.material = wanted.mat;
      const a = Number(wanted.alpha);
      if (wanted.alpha !== null && Number.isFinite(a)) theme.compAlpha = a;
      await new Promise((r) => setTimeout(r, 600));
      window.__PROBE_READY__ = true;
    }, 50);
  }
}
