import { createApp, h, nextTick } from "vue";
import { createRouter, createWebHashHistory } from "vue-router";
import { createPinia } from "pinia";
import "../src/assets/tokens.css";
import "../src/assets/main.css";

/**
 * FEAT-105 结构探针：直接挂真实页面组件，用桩 invoke 喂数据。
 * 断言两件事：① 顶栏里只有本组件内部的分区；② 相册页是双栏（左筛选 + 右内容）。
 */

// 构造 12 个相册（与真实库的分布接近），让页面能渲染出真实密度
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
  if (cmd === "get_albums" || cmd === "list_albums") return ALBUMS;
  if (cmd === "list_timeline" || cmd === "list_persons") return [];
  if (cmd === "get_albums_with_stats") return ALBUMS;
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
  const [{ default: AlbumList }, { default: SmartAlbum }, { default: ScanHub }] = await Promise.all([
    import("../src/views/AlbumList.vue"),
    import("../src/views/SmartAlbum.vue"),
    import("../src/views/ScanHub.vue"),
  ]);

  const cases = [
    { id: "albums", comp: AlbumList, path: "/albums" },
    { id: "smart", comp: SmartAlbum, path: "/smart" },
    { id: "scan", comp: ScanHub, path: "/scan" },
  ];

  for (const c of cases) {
    const host = document.createElement("div");
    host.id = `probe-${c.id}`;
    document.body.appendChild(host);

    const router = createRouter({
      history: createWebHashHistory(),
      routes: [...ROUTES, { path: "/:pathMatch(.*)*", component: { render: () => null } }],
    });
    const app = createApp({ render: () => h(c.comp as any) });
    app.use(createPinia());
    app.use(router);
    await router.isReady();
    await router.push(c.path).catch(() => {});
    app.mount(host);
    await nextTick();
    await new Promise((r) => setTimeout(r, 600));
  }

  (window as any).__PROBE_READY__ = true;
})();
