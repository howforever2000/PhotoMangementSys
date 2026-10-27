import { createApp, h, nextTick } from "vue";
import { createRouter, createWebHashHistory } from "vue-router";
import { createPinia, setActivePinia } from "pinia";
import "../src/assets/tokens.css";
import "../src/assets/widget.css"; /* FEAT-110：概览 widget 墙样式（探针不经过 main.ts，需手动引） */
import "../src/assets/main.css";
import { useThemeStore } from "../src/stores/theme";
import { presetPrefs, DARK_PRESETS, LIGHT_PRESETS } from "../src/utils/presets";

/**
 * FEAT-105 真实主题探针（多页版）
 *
 * URL 形如 `#light/AlbumList` → 浅色预设 + 该页组件。
 * 让 theme store 自己跑一遍 apply，body 上才有正确的 --color-* / --c-*；
 * 只补几个变量的探针会把 #f5f7ff（main.css 深色默认）留在 body 上，浅色页读成白字。
 */

const ALBUMS = [
  ["毕业旅行", 109, 109, "2024-06-15", 1932735283],
  ["聚会合影", 109, 109, "2026-09-02", 1832735283],
  ["美食", 57, 57, "2024-10-08", 993273528],
  ["森林徒步", 42, 42, "2025-05-20", 793273528],
  ["自然风景", 36, 36, "2026-09-11", 693273528],
  ["夜空", 31, 0, "2024-12-03", 593273528],
  ["日落晚霞", 25, 0, "2023-08-19", 493273528],
  ["人像", 24, 24, "2026-09-25", 393273528],
  ["湖与岛", 18, 0, "2025-07-04", 293273528],
  ["猫", 14, 14, "2025-03-12", 193273528],
  ["雪", 12, 0, "2024-01-22", 93273528],
  ["花", 9, 9, "2023-04-07", 43327352],
].map(([name, photo_count, scanned, shoot_time, size_bytes], i) => ({
  id: i + 1,
  name,
  path: `D:/Photos/${name}`,
  description: null,
  cover_path: null,
  created_at: 0,
  updated_at: 0,
  photo_count,
  shoot_time,
  size_bytes,
  scanned_photo_count: scanned,
  location: null,
  tags: [],
  folder_id: null,
  folder_path: "",
  kind: "normal",
  merged_sources: [],
}));

const stub = async (cmd: string) => {
  if (cmd === "get_albums" || cmd === "list_albums" || cmd === "get_albums_with_stats") return ALBUMS;
  if (cmd === "list_timeline" || cmd === "list_persons") return [];
  /* FEAT-110：全局概览新增的统计命令（探针无数据，返回空/零，概览应走空态而非报错） */
  if (cmd === "list_categories") return [];
  if (cmd === "category_index_stats")
    return { indexed: 0, stale: 0, known: 0, model: "", semantic_categories: 0 };
  if (cmd === "get_person_avatars_bulk") return [];
  if (cmd === "get_photo_thumbs") return [];
  /*
   * 地点页需要真实点位才能验证「Ctrl+滚轮缩放」。
   * 字段严格按 LocationGroupRow：lat/lon 是**聚合组中心点**（FEAT-103 加的后端字段），
   * null 表示该组无坐标 → 地图不打点。未记录组固定排最后。
   */
  if (cmd === "list_photo_locations") {
    return [
      { location: "四川省·达州", count: 1751, lat: 31.209, lon: 107.468, cover_path: null, cover_album_id: 1 },
      { location: "四川省·成都", count: 631, lat: 30.572, lon: 104.066, cover_path: null, cover_album_id: 2 },
      { location: "贵州省·贵阳", count: 475, lat: 26.647, lon: 106.630, cover_path: null, cover_album_id: 3 },
      { location: "浙江省·杭州", count: 375, lat: 30.274, lon: 120.155, cover_path: null, cover_album_id: 4 },
      { location: "重庆市", count: 402, lat: 29.563, lon: 106.551, cover_path: null, cover_album_id: 5 },
      { location: "广东省·深圳", count: 81, lat: 22.543, lon: 114.058, cover_path: null, cover_album_id: 6 },
      { location: "上海市", count: 63, lat: 31.230, lon: 121.474, cover_path: null, cover_album_id: 7 },
      { location: "北京市", count: 44, lat: 39.904, lon: 116.407, cover_path: null, cover_album_id: 8 },
      { location: "陕西省·西安", count: 38, lat: 34.341, lon: 108.940, cover_path: null, cover_album_id: 9 },
      { location: "云南省·昆明", count: 27, lat: 25.038, lon: 102.718, cover_path: null, cover_album_id: 10 },
      { location: "黑龙江省·哈尔滨", count: 19, lat: 45.803, lon: 126.535, cover_path: null, cover_album_id: 11 },
      { location: "新疆维吾尔自治区·乌鲁木齐", count: 12, lat: 43.826, lon: 87.617, cover_path: null, cover_album_id: 12 },
      { location: null, count: 14999, lat: null, lon: null, cover_path: null, cover_album_id: null },
    ];
  }
  if (cmd === "search_photo_content" || cmd === "search_albums") return [];
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

const PAGE_PATH: Record<string, string> = {
  Home: "/home",
  AlbumList: "/albums",
  SmartAlbum: "/smart",
  ScanHub: "/scan",
  Memories: "/memories",
  Timeline: "/timeline",
  Workshop: "/workshop",
};

(async () => {
  const [which, pageName = "AlbumList"] = location.hash.replace(/^#/, "").split("/");
  const mode = which === "light" ? "light" : "dark";
  /* 按 id 取而不是按下标：FEAT-119 把浅色从 4 套砍到 2 套，
     原来的 LIGHT_PRESETS[2] 直接变成 undefined，审计脚本静默报错、
     还照着空预设算出"0 处不合格" —— 探针本身也要有"取不到就大声失败"。 */
  const LIGHT_PROBE_ID = "celadon-light";
  const preset =
    mode === "light"
      ? (LIGHT_PRESETS.find((p) => p.id === LIGHT_PROBE_ID) ?? LIGHT_PRESETS[0])
      : DARK_PRESETS[0];
  if (!preset) throw new Error(`预设缺失：mode=${mode}，请检查 src/utils/presets.ts`);

  const mod = await import(/* @vite-ignore */ `/src/views/${pageName}.vue`);
  const Comp = mod.default;

  const host = document.createElement("div");
  host.id = `probe-${mode}`;
  document.body.appendChild(host);

  /*
   * FEAT-108：液态玻璃的 SVG 滤镜定义必须挂到探针页上。
   * 真实应用里它由 App.vue 根的 <GlassFilters /> 提供，
   * 但探针页只挂单个页面组件（不经过 App.vue）——
   * 不补这一步的话，顶栏的 `backdrop-filter: url(#liquid-lens)`
   * 引不到任何滤镜，折射会静默失效（实测：与纯模糊像素差 0）。
   */
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
  const patch = presetPrefs(preset);
  for (const k of ["bgStyle", "gradFrom", "gradMid", "gradTo", "gradAngle", "compColor", "compAlpha", "material"] as const) {
    const v = (patch as any)[k];
    if (v !== undefined) (theme as any)[k] = v;
  }
  if (patch.mode) (theme as any).mode = patch.mode;
  await nextTick();
  await new Promise((r) => setTimeout(r, 400));

  await router.isReady();
  await router.push(PAGE_PATH[pageName] ?? "/albums").catch(() => {});
  app.mount(host);
  await nextTick();
  await new Promise((r) => setTimeout(r, 800));

  (window as any).__PROBE_READY__ = true;
  (window as any).__PRESET__ = { id: preset.id, mode: preset.mode, colors: preset.colors, page: pageName };
})();
