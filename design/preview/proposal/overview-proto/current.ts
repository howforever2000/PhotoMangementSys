// 临时验证夹具：用真实 SmartAlbum / OverviewPane 渲染「全局概览」，供 playwright 截图。
// 不参与构建，验证完即删。
import { createApp, h } from "vue";
import { createPinia } from "pinia";
import { createRouter, createMemoryHistory, RouterView } from "vue-router";
import "../../../../src/assets/tokens.css";
import "../../../../src/assets/widget.css";
import "../../../../src/assets/main.css";
import "../../../../src/assets/pm-dialog.css";

const PHOTO = "/design/preview/proposal/lightbox-photo.svg";

/* ── 模拟后端 ─────────────────────────────────────────── */
const albums = [
  { id: 1, name: "大理 · 洱海", photo_count: 1284, size_bytes: 9.2e9 },
  { id: 2, name: "2026 春 · 无锡", photo_count: 942, size_bytes: 6.4e9 },
  { id: 3, name: "家庭日常", photo_count: 5310, size_bytes: 31e9 },
  { id: 4, name: "婚礼 · 阿哲", photo_count: 2410, size_bytes: 18e9 },
  { id: 5, name: "川西环线", photo_count: 1866, size_bytes: 14e9 },
  { id: 6, name: "扫街 · 上海", photo_count: 712, size_bytes: 4.1e9 },
  { id: 7, name: "女儿成长", photo_count: 3890, size_bytes: 21e9 },
  { id: 8, name: "2025 冬 · 哈尔滨", photo_count: 640, size_bytes: 4.8e9 },
  { id: 9, name: "老照片扫描", photo_count: 3208, size_bytes: 12e9 },
  { id: 10, name: "工作素材", photo_count: 1820, size_bytes: 9.6e9 },
  { id: 11, name: "朋友合影", photo_count: 2064, size_bytes: 11e9 },
  { id: 12, name: "旅行 · 日本", photo_count: 834, size_bytes: 6.2e9 },
].map((a, i) => ({
  ...a,
  path: `/photos/album-${a.id}`,
  description: null,
  cover_path: PHOTO,
  created_at: 1700000000,
  updated_at: 1760000000,
  shoot_time: "2026-10-20",
  scanned_photo_count: i % 3 === 0 ? 0 : Math.round(a.photo_count * 0.8),
  location: null,
}));

const months: Array<[string, number]> = [];
for (let y = 2018; y <= 2026; y++) {
  for (let m = 1; m <= 12; m++) {
    if (y === 2026 && m > 10) continue;
    const base = y < 2022 ? 60 : y < 2025 ? 220 : 380;
    months.push([`${y}-${String(m).padStart(2, "0")}`, base + ((m * 37 + y * 11) % 260)]);
  }
}

const stats = {
  total_scanned: 18420,
  no_time: 410,
  with_face: 11260,
  with_gps: 7860,
  year_min: 2015,
  year_max: 2026,
  months,
  recent: Array.from({ length: 6 }, (_, i) => ({
    path: PHOTO,
    album_id: (i % 6) + 1,
    album_name: albums[i % 6].name,
    shoot_time: `2026-10-2${i}`,
    location: "云南省 大理市",
  })),
  on_this_day: {
    path: PHOTO,
    album_id: 2,
    album_name: "2026 春 · 无锡",
    shoot_time: "2023-10-30 15:20:00",
    location: "江苏省 无锡市",
  },
  on_this_day_count: 7,
  indexed: 17230,
  stale: 118,
  known: 18420,
  semantic_categories: 9,
  model: "clip-vit-b32",
  signature: "mock",
  built_at: Date.now(),
};

const persons = [
  { id: "P001", name: "阿哲", face_count: 1840 },
  { id: "P002", name: "小雨", face_count: 1620 },
  { id: "P003", name: "妈妈", face_count: 1310 },
  { id: "P004", name: "爸爸", face_count: 1120 },
  { id: "P005", name: "团子", face_count: 980 },
  { id: "P006", name: null, face_count: 760 },
  { id: "P007", name: null, face_count: 640 },
  { id: "P008", name: "外婆", face_count: 520 },
  { id: "P009", name: null, face_count: 410 },
  { id: "P010", name: null, face_count: 380 },
  { id: "P011", name: "小舅", face_count: 300 },
  { id: "P012", name: null, face_count: 240 },
].map((p) => ({ ...p, created_at: "2026-01-01" }));

const locations = [
  ["中国·北京市·朝阳区", 2140],
  ["中国·云南省·大理市", 1860],
  ["中国·上海市·徐汇区", 1420],
  ["中国·四川省·成都市", 980],
  ["中国·江苏省·无锡市", 760],
  ["中国·黑龙江省·哈尔滨市", 620],
  ["中国·浙江省·杭州市", 540],
  ["中国·广东省·深圳市", 380],
  [null, 4210],
].map(([location, count]) => ({
  location: location as string | null,
  count: count as number,
  cover_path: PHOTO,
  cover_album_id: 1,
  lat: null,
  lon: null,
}));

const categories = [
  { id: 1, name: "人物", icon: "🧑", source: "builtin", slug: "portrait", threshold: 0.3, enabled: true, keywords: [], exclude_keywords: [], count: 11260 },
  { id: 2, name: "扫街", icon: "🏙", source: "builtin", slug: "street", threshold: 0.3, enabled: true, keywords: [], exclude_keywords: [], count: 820 },
  { id: 3, name: "夜景", icon: "🌃", source: "builtin", slug: "night_scene", threshold: 0.3, enabled: true, keywords: [], exclude_keywords: [], count: 640 },
  { id: 4, name: "文档", icon: "📄", source: "builtin", slug: "document", threshold: 0.3, enabled: false, keywords: [], exclude_keywords: [], count: 90 },
];

const MOCK: Record<string, unknown> = {
  get_albums: albums,
  overview_stats: stats,
  list_persons: persons,
  list_photo_locations: locations,
  list_categories: categories,
  get_person_avatars_bulk: ["/design/preview/proposal/lightbox-photo.svg"],
};

(window as any).__TAURI_INTERNALS__ = {
  convertFileSrc: (p: string) => p,
  transformCallback: () => 0,
  invoke: (cmd: string, args?: any) => {
    if (cmd === "get_photo_thumbs") {
      // 返回每张照片自己的缩略图（用占位图代替真图）
      return Promise.resolve((args?.paths ?? []).map((p: string) => [p, PHOTO]));
    }
    if (cmd === "get_person_avatars_bulk") {
      return Promise.resolve((args?.pids ?? []).map(() => PHOTO));
    }
    if (cmd in MOCK) return Promise.resolve(MOCK[cmd]);
    return Promise.reject(new Error(`harness: 未模拟的命令 ${cmd}`));
  },
};

const { default: SmartAlbum } = await import("../../../../src/views/SmartAlbum.vue");

const router = createRouter({
  history: createMemoryHistory(),
  routes: [
    { path: "/smart", component: SmartAlbum },
    { path: "/albums", component: { template: "<div/>" } },
    { path: "/timeline", component: { template: "<div/>" } },
    { path: "/memories", component: { template: "<div/>" } },
    { path: "/search", component: { template: "<div/>" } },
    { path: "/album/:id", component: { template: "<div/>" } },
    { path: "/", component: { template: "<div/>" } },
  ],
});

const app = createApp({ render: () => h(RouterView) });
app.use(createPinia());
app.use(router);
await router.push("/smart");
await router.isReady();
app.mount("#app");
