import { createApp, h, nextTick } from "vue";
import { createRouter, createWebHashHistory } from "vue-router";
import { createPinia, setActivePinia } from "pinia";
import "../src/assets/tokens.css";
import "../src/assets/widget.css";
import "../src/assets/main.css";
import { useThemeStore } from "../src/stores/theme";
import { DARK_PRESETS } from "../src/utils/presets";

/**
 * FEAT-110 全局概览探针
 * URL hash：#dusk|pine|plain/SmartAlbum
 * 用真实 SmartAlbum 页面（含 AppHeader + OverviewPane），mock 全部统计命令。
 */

const ASSETS = ["/favicon.png", "/tauri.svg", "/vite.svg"];
const ALBUM_NAMES = ["夜空", "美食", "森林徒步", "自然风景", "聚会合影", "花", "日落晚霞", "人像", "湖与岛", "猫", "雪", "街景"];

/* 照片总量与已入库刻意对齐 99.6%（2507 总 / 2498 已入库） */
const TOTAL = 2507;
const SCANNED = 2498;

const ALBUMS = ALBUM_NAMES.map((name, i) => {
  const count = i === 0 ? TOTAL - (ALBUM_NAMES.length - 1) * 200 : 200;
  return {
    id: i + 1,
    name,
    path: `D:/Photos/${name}`,
    description: null,
    cover_path: ASSETS[i % ASSETS.length],
    created_at: 0,
    updated_at: 0,
    photo_count: count,
    shoot_time: null,
    size_bytes: Math.round(81.3 * 1024 ** 3 / ALBUM_NAMES.length),
    scanned_photo_count: Math.round(count * 0.996),
    location: null,
    tags: [],
    folder_id: null,
    folder_path: "",
    kind: "normal",
    merged_sources: [],
  };
});

const CITIES: [string, number, number][] = [
  ["四川省·成都", 30.572, 104.066],
  ["四川省·达州", 31.209, 107.468],
  ["重庆市", 29.563, 106.551],
  ["浙江省·杭州", 30.274, 120.155],
  ["贵州省·贵阳", 26.647, 106.63],
  ["广东省·深圳", 22.543, 114.058],
  ["陕西省·西安", 34.341, 108.94],
  ["云南省·昆明", 25.038, 102.718],
  ["上海市", 31.23, 121.474],
  ["北京市", 39.904, 116.407],
];

/* 生成已入库时间线（含跨年月度分布 + 今日同月日回忆 + GPS/人物） */
const now = new Date();
const MM = String(now.getMonth() + 1).padStart(2, "0");
const DD = String(now.getDate()).padStart(2, "0");
const timeline: any[] = [];
let id = 1;
function pushRow(y: number, m: number, d: number, withLoc: boolean, withFace: boolean) {
  const albumIdx = (y + m + d) % ALBUM_NAMES.length;
  const name = ALBUM_NAMES[albumIdx];
  const city = CITIES[(y * 31 + m * 7 + d) % CITIES.length];
  timeline.push({
    id: id++,
    path: `D:/Photos/${name}/${y}${String(m).padStart(2, "0")}${String(d).padStart(2, "0")}_${id}.jpg`,
    parent_dir: `D:/Photos/${name}`,
    album_id: albumIdx + 1,
    album_name: name,
    album_path: `D:/Photos/${name}`,
    content: "",
    category: name,
    sub_category: null,
    label: null,
    confidence: null,
    person_ids: withFace ? ["P001", "P002"] : [],
    shoot_time: `${y}-${String(m).padStart(2, "0")}-${String(d).padStart(2, "0")} 12:00:00`,
    location: withLoc ? city[0] : null,
    iso: "100",
    aperture: "2.8",
    shutter_speed: "1/120",
    focal_length: "35",
  });
}
(function gen() {
  /* 2017–2025 每月若干，制造逐年增长 */
  for (let y = 2017; y <= 2025; y++) {
    const perMonth = 6 + (y - 2017) * 3;
    for (let m = 1; m <= 12; m++) {
      for (let k = 0; k < perMonth; k++) {
        pushRow(y, m, 1 + ((k * 7 + m * 3) % 27) + 1, (k + m) % 5 === 0, (k + m) % 4 === 0);
      }
    }
  }
  /* 2026 年到当月：当月 20 张 */
  for (let m = 1; m <= now.getMonth() + 1; m++) {
    const n = m === now.getMonth() + 1 ? 20 : 30 + m * 6;
    for (let k = 0; k < n; k++) {
      pushRow(2026, m, 1 + ((k * 5 + m) % 27) + 1, (k + m) % 3 === 0, (k + m) % 3 !== 0);
    }
  }
  /* 往年同月同日：保证「那年今日」非空（5 个年份） */
  [2019, 2020, 2021, 2022, 2023].forEach((y, i) => pushRow(y, now.getMonth() + 1, now.getDate(), i % 2 === 0, true));
  /* 截断/补齐到 SCANNED。多了从**头部（最老 2017）**删，保住 2026 当月与最近导入；
     少了补在 2024（较老），同样不污染“最近”。 */
  while (timeline.length > SCANNED) timeline.shift();
  let guard = 0;
  while (timeline.length < SCANNED && guard++ < 3000) {
    pushRow(2024, 1 + (guard % 12), 1 + (guard % 27) + 1, guard % 3 === 0, guard % 2 === 0);
  }
  timeline.sort((a, b) => a.shoot_time.localeCompare(b.shoot_time));
})();

const persons = Array.from({ length: 12 }, (_, i) => ({
  id: `P00${i + 1}`,
  name: ["", "妈妈", "爸爸", "", "外婆", "", "", "", "", "", "", ""][i] || `P00${i + 1}`,
  face_count: 900 - i * 60,
  created_at: "",
}));

const locations = [
  { location: "四川省·成都", count: 639, lat: 30.572, lon: 104.066, cover_path: null, cover_album_id: 1 },
  { location: "四川省·达州", count: 175, lat: 31.209, lon: 107.468, cover_path: null, cover_album_id: 1 },
  { location: "重庆市", count: 40, lat: 29.563, lon: 106.551, cover_path: null, cover_album_id: 5 },
  { location: "浙江省·杭州", count: 37, lat: 30.274, lon: 120.155, cover_path: null, cover_album_id: 4 },
  { location: "贵州省·贵阳", count: 47, lat: 26.647, lon: 106.63, cover_path: null, cover_album_id: 3 },
  { location: "广东省·深圳", count: 21, lat: 22.543, lon: 114.058, cover_path: null, cover_album_id: 6 },
  { location: "陕西省·西安", count: 16, lat: 34.341, lon: 108.94, cover_path: null, cover_album_id: 9 },
  { location: "云南省·昆明", count: 12, lat: 25.038, lon: 102.718, cover_path: null, cover_album_id: 10 },
  { location: null, count: SCANNED - 987, lat: null, lon: null, cover_path: null, cover_album_id: null },
];

const categories = ["夜空", "美食", "森林", "自然风景", "聚会合影", "花", "日落晚霞", "人像", "湖与岛", "猫", "雪", "街景"]
  .map((name, i) => ({
    id: i + 1,
    name,
    icon: "🏷",
    source: "preset",
    slug: "",
    threshold: 0.2,
    enabled: true,
    keywords: [name],
    exclude_keywords: [],
    count: 240 - i * 12,
    cover_path: ASSETS[i % ASSETS.length],
    cover_album_id: (i % ALBUM_NAMES.length) + 1,
    cover_photo_hash: null,
  }));

function assetForPath(p: string) {
  const idx = ALBUM_NAMES.findIndex((n) => p.includes(`/${n}/`));
  return ASSETS[(idx < 0 ? 0 : idx) % ASSETS.length];
}

const stub = async (cmd: string, args?: any) => {
  if (cmd === "get_albums") return ALBUMS;
  if (cmd === "list_timeline") return timeline;
  if (cmd === "list_persons") return persons;
  if (cmd === "list_photo_locations") return locations;
  if (cmd === "list_categories") return categories;
  if (cmd === "category_index_stats")
    return { indexed: SCANNED - 9, stale: 9, known: SCANNED, model: "cn-clip-b16", semantic_categories: 36 };
  if (cmd === "get_person_avatars_bulk")
    return (args?.pids ?? []).map((p: string) => {
      const i = Number(p.replace("P00", "")) - 1;
      return i % 2 === 0 ? ASSETS[i % ASSETS.length] : null;
    });
  if (cmd === "get_photo_thumbs")
    return (args?.paths ?? []).map((p: string) => [p, assetForPath(p)]);
  return null;
};
(window as any).__TAURI_INTERNALS__ = {
  invoke: stub,
  transformCallback: (cb: any) => cb,
  convertFileSrc: (p: string) => p,
};
(window as any).__TAURI__ = {
  core: { invoke: stub, convertFileSrc: (p: string) => p },
  event: { listen: async () => () => {}, emit: async () => {} },
  dialog: { open: async () => null },
};

const ROUTES = [
  "/", "/home", "/albums", "/smart", "/scan", "/scan/test",
  "/timeline", "/memories", "/search", "/workshop", "/album/:id",
].map((path) => ({ path, component: { render: () => null } }));

(async () => {
  const [which] = location.hash.replace(/^#/, "").split("/");
  const variant = which || "dusk";

  const mod = await import(/* @vite-ignore */ `/src/views/SmartAlbum.vue`);
  const Comp = mod.default;

  const host = document.createElement("div");
  host.id = `probe-${variant}`;
  document.body.appendChild(host);

  const filterHost = document.createElement("div");
  document.body.appendChild(filterHost);
  const { default: GlassFilters } = await import("/src/components/ui/GlassFilters.vue");
  createApp({ render: () => h(GlassFilters) }).mount(filterHost);

  const router = createRouter({
    history: createWebHashHistory(),
    routes: [...ROUTES, { path: "/:pathMatch(.*)*", component: { render: () => null } }],
  });
  const pinia = createPinia();
  const app = createApp({ render: () => h(Comp as any) });
  app.use(pinia);
  app.use(router);
  setActivePinia(pinia);

  const theme = useThemeStore();
  const pine = DARK_PRESETS.find((p) => p.id === "pine-night")!;
  theme.applyPreset(variant === "pine" ? pine : DARK_PRESETS[0]);
  if (variant === "plain") theme.widgetMaterial = "plain";
  /* 探针不挂 App.vue：手动把主题底层渐变铺到 body（真实应用由 .app-bg-base 提供） */
  await nextTick();
  document.body.style.background = (theme.layerBase as any).background;
  document.body.style.minHeight = "100vh";

  await router.isReady();
  await router.push("/smart").catch(() => {});
  app.mount(host);
  await nextTick();
  await new Promise((r) => setTimeout(r), 1400);

  (window as any).__PROBE_READY__ = true;
  (window as any).__VARIANT__ = variant;
})();
