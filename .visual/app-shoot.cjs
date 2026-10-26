// 真实页面实拍（App + 路由 + Tauri 桩数据）
// 用法：node .visual/app-shoot.cjs [presetId] [route ...]
//   例：node .visual/app-shoot.cjs sunset-dusk /albums /smart
//   预设取自 src/utils/presets.ts（运行时动态 import，永远与代码同步）
const { chromium } = require("playwright");
const fs = require("fs");
const PORT = process.env.VITE_PORT || "1519";
const OUT = ".visual/out/pages";
fs.mkdirSync(OUT, { recursive: true });

const PRESETS = ["sunset-dusk", "ink-slate", "ember-warm", "pine-night", "linen-light", "mist-light", "paper-light", "celadon-light", "lilac-light", "clay-light", "default-look"];

const argv = process.argv.slice(2);
let presetId = "sunset-dusk";
let routes = ["/albums", "/timeline", "/memories", "/smart", "/scan", "/search", "/album/1", "/workshop"];
if (argv.length) {
  if (!argv[0].startsWith("/")) { presetId = argv.shift(); }
  if (argv.length) routes = argv;
}
if (presetId === "all") presetId = "sunset-dusk";

/* ── 桩数据：与 overview-probe 同一套口径（真实感优先） ── */
const INIT = ({ photos, albums, timeline, persons, categories, locations, trees }) => {
  const stub = async (cmd, args) => {
    switch (cmd) {
      case "get_current_user": return { id: 1, username: "hao", email: "t@example.com", phone: "13800000000", created_at: 1700000000, avatar: null };
      case "get_albums": return albums;
      case "get_album": return albums.find((a) => a.id === Number(args?.id)) ?? albums[0];
      case "list_timeline": return timeline;
      case "list_persons": return persons;
      case "get_person_avatars_bulk": {
        const pids = args?.pids ?? [];
        return pids.map((_, i) => photos[i % photos.length]);
      }
      case "get_person_avatar": return photos[0];
      case "get_person_photos": return timeline.slice(0, 40);
      case "get_photo_thumbs": return (args?.paths ?? []).map((p, i) => [p, photos[i % photos.length]]);
      case "prewarm_thumbs": return { requested: 1, hit: 1, generated: 0, failed: 0 };
      case "list_album_photos": return timeline.slice(0, 60).map((r) => r.path);
      case "get_photo_ratings": return (args?.paths ?? []).map((p) => ({ path: p, rating: 0 }));
      case "get_photo_mark_states": return (args?.paths ?? []).map((p) => ({ path: p, state: null }));
      case "list_categories": return categories;
      case "category_index_stats": return { indexed: 2388, stale: 9, known: 2400, model: "cn-clip-b16", semantic_categories: 36 };
      case "list_category_photos": return timeline.slice(0, 48);
      case "list_photo_locations": return locations;
      case "list_photos_by_location": return timeline.slice(0, 24);
      case "get_manual_tree": return trees;
      case "list_marked_photos": return [];
      case "list_recently_deleted": return [];
      case "get_folders": return trees;
      case "list_folders": return trees;
      case "get_scan_perf_settings": return null;
      case "scan_all_status": return { running: false, done: 0, total: 0 };
      case "get_model_status": return [];
      case "get_hardware_info": return null;
      default: return null;
    }
  };
  window.__TAURI_INTERNALS__ = {
    invoke: stub,
    transformCallback: (cb) => cb,
    convertFileSrc: (p) => p,
    metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
  };
  window.__TAURI__ = {
    core: { invoke: stub, convertFileSrc: (p) => p },
    event: { listen: async () => () => {}, emit: async () => {} },
    dialog: { open: async () => null },
  };
};

(async () => {
  const base = `http://localhost:${PORT}`;
  const b = await chromium.launch({ channel: "msedge" });
  for (const route of routes) {
    const p = await b.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });
    p.on("pageerror", (e) => console.log(`  [${route}] pageerror`, String(e).split("\n")[0]));
    p.on("console", (m) => { if (m.type() === "error") console.log(`  [${route}] console.error`, m.text().slice(0, 160)); });

    await p.addInitScript(INIT, buildData());
    /* vite 冷启动后的第一次导航偶发「白屏」（模块图还没就绪 → 入口脚本 404）。
       等待 .app-shell 出现，失败则重载一次，避免拍出一张全白图。 */
    for (let attempt = 0; attempt < 3; attempt++) {
      await p.goto(`${base}/home`, { waitUntil: "load" });
      const ok = await p.waitForSelector(".app-shell", { timeout: 12000 }).then(() => true).catch(() => false);
      if (ok) break;
      console.log(`  [${route}] 首屏未挂载，重试 ${attempt + 1}`);
      await p.waitForTimeout(1500);
    }

    /* 预设：运行时从源码取，避免手抄色值 */
    const patch = await p.evaluate(async (id) => {
      const m = await import("/src/utils/presets.ts");
      if (id === "default-look") { localStorage.removeItem("pm-theme"); return { bgStyle: "image" }; }
      const pr = [...m.DARK_PRESETS, ...m.LIGHT_PRESETS].find((x) => x.id === id);
      if (!pr) return null;
      localStorage.setItem("pm-theme", JSON.stringify(m.presetPrefs(pr)));
      return m.presetPrefs(pr);
    }, presetId);
    if (!patch) { console.log("未知预设", presetId); }
    await p.reload({ waitUntil: "load" });
    await p.goto(`${base}${route}`, { waitUntil: "load" });
    await p.waitForSelector(".app-shell", { timeout: 15000 }).catch(() => {});
    await p.evaluate(() => document.fonts.ready);
    await p.waitForTimeout(1600);
    const name = `${presetId}${route.replace(/[/?=&]/g, "_")}`;
    await p.screenshot({ path: `${OUT}/${name}.png` });
    console.log("shot", `${OUT}/${name}.png`);
    await p.close();
  }
  await b.close();
})();

/* 数据构造放在 Node 侧，通过 addInitScript 参数注入 */
function buildData() {
  const photos = Array.from({ length: 12 }, (_, i) => `/design/preview/assets/p${i + 1}.jpg`);
  const NAMES = ["夜空", "美食", "森林徒步", "自然风景", "聚会合影", "花", "日落晚霞", "人像", "湖与岛", "猫", "雪", "街景"];
  const TOTAL = 2507;
  const albums = NAMES.map((name, i) => {
    const count = i === 0 ? TOTAL - (NAMES.length - 1) * 200 : 200;
    return {
      id: i + 1, name, path: `D:/Photos/${name}`, description: null,
      cover_path: photos[i % photos.length], created_at: 0, updated_at: 0,
      photo_count: count, shoot_time: `${2026 - (i % 6)}-06-15`,
      size_bytes: Math.round((81.3 * 1024 ** 3) / NAMES.length),
      scanned_photo_count: Math.round(count * 0.996),
      location: null, tags: [], folder_id: null, folder_path: "", kind: "normal", merged_sources: [],
    };
  });

  const CITIES = [["四川省·成都", 30.57, 104.07], ["四川省·达州", 31.2, 107.47], ["重庆市", 29.56, 106.55], ["浙江省·杭州", 30.27, 120.15], ["贵州省·贵阳", 26.65, 106.63], ["广东省·深圳", 22.54, 114.06], ["陕西省·西安", 34.34, 108.94], ["云南省·昆明", 25.04, 102.72]];
  const now = new Date();
  const timeline = [];
  let id = 1;
  const push = (y, m, d, withLoc, withFace) => {
    const ai = (y + m + d) % NAMES.length;
    const c = CITIES[(y * 31 + m * 7 + d) % CITIES.length];
    timeline.push({
      id: id++, path: photos[id % photos.length], parent_dir: `D:/Photos/${NAMES[ai]}`,
      album_id: ai + 1, album_name: NAMES[ai], album_path: `D:/Photos/${NAMES[ai]}`,
      content: "", category: NAMES[ai], sub_category: null, label: null, confidence: null,
      person_ids: withFace ? ["P001", "P002"] : [],
      shoot_time: `${y}-${String(m).padStart(2, "0")}-${String(d).padStart(2, "0")} 12:00:00`,
      location: withLoc ? c[0] : null, iso: "100", aperture: "2.8", shutter_speed: "1/120", focal_length: "35",
    });
  };
  for (let y = 2019; y <= 2025; y++) {
    const per = 8 + (y - 2019) * 4;
    for (let m = 1; m <= 12; m++) for (let k = 0; k < per; k++) push(y, m, 1 + ((k * 7 + m * 3) % 27), (k + m) % 5 === 0, (k + m) % 4 === 0);
  }
  for (let m = 1; m <= now.getMonth() + 1; m++) {
    const n = m === now.getMonth() + 1 ? 24 : 30 + m * 6;
    for (let k = 0; k < n; k++) push(2026, m, 1 + ((k * 5 + m) % 27), (k + m) % 3 === 0, (k + m) % 3 !== 0);
  }
  [2020, 2021, 2022, 2023, 2024].forEach((y, i) => push(y, now.getMonth() + 1, now.getDate(), i % 2 === 0, true));
  timeline.sort((a, b) => a.shoot_time.localeCompare(b.shoot_time));

  const persons = Array.from({ length: 12 }, (_, i) => ({
    id: `P00${i + 1}`, name: ["", "妈妈", "爸爸", "", "外婆", "", "", "", "", "", "", ""][i] || `人物 ${i + 1}`,
    face_count: 900 - i * 60, created_at: "", avatar_path: i % 2 ? null : photos[i % photos.length],
  }));

  const locations = [
    { location: "四川省·成都", count: 639, lat: 30.57, lon: 104.07, cover_path: photos[0], cover_album_id: 1 },
    { location: "四川省·达州", count: 175, lat: 31.2, lon: 107.47, cover_path: photos[1], cover_album_id: 1 },
    { location: "重庆市", count: 40, lat: 29.56, lon: 106.55, cover_path: photos[2], cover_album_id: 5 },
    { location: "浙江省·杭州", count: 37, lat: 30.27, lon: 120.15, cover_path: photos[3], cover_album_id: 4 },
    { location: "贵州省·贵阳", count: 47, lat: 26.65, lon: 106.63, cover_path: photos[4], cover_album_id: 3 },
    { location: "广东省·深圳", count: 21, lat: 22.54, lon: 114.06, cover_path: photos[5], cover_album_id: 6 },
    { location: "陕西省·西安", count: 16, lat: 34.34, lon: 108.94, cover_path: photos[6], cover_album_id: 9 },
    { location: "云南省·昆明", count: 12, lat: 25.04, lon: 102.72, cover_path: photos[7], cover_album_id: 10 },
    { location: null, count: 1520, lat: null, lon: null, cover_path: null, cover_album_id: null },
  ];

  const categories = NAMES.map((name, i) => ({
    id: i + 1, name, icon: "", source: "preset", slug: "", threshold: 0.2, enabled: true,
    keywords: [name], exclude_keywords: [], count: 240 - i * 12,
    cover_path: photos[i % photos.length], cover_album_id: (i % NAMES.length) + 1, cover_photo_hash: null,
  }));

  const trees = [{ id: 1, name: "家庭", parent_id: null, album_count: 4 }, { id: 2, name: "旅行", parent_id: null, album_count: 3 }, { id: 3, name: "创作", parent_id: null, album_count: 2 }];
  return { photos, albums, timeline, persons, categories, locations, trees };
}
