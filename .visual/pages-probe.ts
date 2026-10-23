import { createApp, h } from "vue";
import "../src/assets/tokens.css";
import "../src/assets/main.css";
import StatBand from "../src/components/ui/StatBand.vue";
import GroupStat from "../src/components/ui/GroupStat.vue";
import AlbumCard from "../src/components/AlbumCard.vue";
import type { Album } from "../src/types/album";

// 构造符合 Album 接口的最小数据（封面用 CSS 渐变占位，避免依赖真实图片）
function mk(id: number, name: string, n: number, y: string, scanned: boolean): Album {
  return {
    id, name, path: `/photos/${name}`, description: null, cover_path: null,
    created_at: 0, updated_at: 0, photo_count: n, shoot_time: `${y}-06-15`,
    size_bytes: n * 4.2 * 1024 * 1024, scanned_photo_count: scanned ? n : 0,
    location: null, tags: [], folder_id: null, folder_path: "", kind: "normal",
    merged_sources: [],
  };
}

const albums = [
  mk(1, "2026 毕业季", 108, "2026", true),
  mk(2, "家人", 84, "2025", true),
  mk(3, "猫咪日常", 36, "2026", false),
  mk(4, "秋日街头", 62, "2026", true),
  mk(5, "毕业合影", 109, "2026", true),
  mk(6, "落日山峦", 28, "2026", false),
  mk(7, "海边", 41, "2025", true),
  mk(8, "夜景拾遗", 23, "2024", false),
];

const stats = [
  { label: "相册", value: 12, delta: "+2 系统" },
  { label: "照片", value: 653 },
  { label: "已入库", value: 615, delta: "10 个相册", tone: "up" as const },
  { label: "待入库", value: 2, delta: "需要扫描", tone: "up" as const },
  { label: "占用", value: "18.4", unit: "GB" },
];


createApp({
  setup() {
    return () =>
      h("div", { style: "min-height:100vh;background:var(--c-bg);padding-top:26px" }, [
        h(StatBand, { items: stats }),
        h("div", { style: "height:1px;background:var(--c-rule);margin:0" }),
        // 年组头（带组级统计）
        h("div", { style: "padding:14px 52px 0;display:flex;align-items:center;gap:10px" }, [
          h("span", { style: "font-size:var(--fs-head);font-weight:var(--fw-em);color:var(--c-fg)" }, "🗓️ 2026 年"),
          h("span", { style: "font-size:var(--fs-meta);color:var(--c-fg-2)" }, "388 张"),
          h(GroupStat, {
            class: "tl-year-stat",
            style: "padding-left:12px;border-left:1px solid var(--c-rule)",
            items: [{ value: 5, label: "个月" }, { value: 78, label: "平均/月" }],
            dist: [0,0,0,0,71,96,108,0,0,0,0,0],
            distHighlight: 6,
          }),
        ]),
        // tile 网格：直接用真实组件（scoped 样式只有组件自己才带）
        h("div", { style: "padding:18px 52px;display:grid;grid-template-columns:repeat(auto-fill,minmax(216px,1fr));gap:30px 26px" },
          albums.map((a) => h(AlbumCard, { key: a.id, album: a, selectMode: false, selected: false, variant: "tile" })),
        ),
      ]);
  },
}).mount("#app");
