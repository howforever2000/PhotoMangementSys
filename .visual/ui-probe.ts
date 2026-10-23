import { createApp, h } from "vue";
import "../src/assets/tokens.css";
import StatBand from "../src/components/ui/StatBand.vue";
import GroupStat from "../src/components/ui/GroupStat.vue";

const statItems = [
  { label: "照片总数", value: 653, delta: "+28", tone: "up" as const, spark: [34,52,41,66,96,57,44] },
  { label: "相册", value: 12, delta: "持平", tone: "mute" as const, spark: [60,60,72,72,72,72,72] },
  { label: "人物聚类", value: 170, delta: "3 待命名", tone: "mute" as const, spark: [28,44,58,70,82,90,96] },
  { label: "语义分类", value: 21, delta: "+2", tone: "up" as const, spark: [38,38,55,55,74,74,92] },
  { label: "占用 / 扫描", value: "18.4", unit: "GB", delta: "92s ↓31%", tone: "up" as const, spark: [92,86,78,70,60,52,44] },
];
const albums = [
  { name: "2026 毕业季", meta: "108 · 人物 42", badge: "已扫描" },
  { name: "家人", meta: "84 · 2025" },
  { name: "猫咪日常", meta: "36 · 语义" },
  { name: "秋日街头", meta: "62 · 2026" },
  { name: "毕业合影", meta: "109 · 人物 28" },
  { name: "落日山峦", meta: "28 · 2026" },
];
// 用渐变色块充当封面（真实项目用 convertFileSrc 的图片 URL）
const grads = [
  "radial-gradient(112% 84% at 24% 12%,#e6c79b 0%,rgba(230,199,155,0) 48%),linear-gradient(152deg,#7a6a9c 0%,#463c5c 44%,#1b1724 100%)",
  "radial-gradient(100% 80% at 72% 16%,#a8d8cd 0%,rgba(168,216,205,0) 50%),linear-gradient(152deg,#3f7c74 0%,#274a52 46%,#131e24 100%)",
  "radial-gradient(100% 80% at 36% 18%,#e3b6c8 0%,rgba(227,182,200,0) 48%),linear-gradient(152deg,#8f5a70 0%,#543646 46%,#241821 100%)",
  "radial-gradient(100% 80% at 62% 16%,#e2cfa0 0%,rgba(226,207,160,0) 48%),linear-gradient(152deg,#8a7248 0%,#4e4331 46%,#221e18 100%)",
  "radial-gradient(100% 80% at 42% 18%,#c3ccec 0%,rgba(195,204,236,0) 50%),linear-gradient(152deg,#4e5980 0%,#31384f 46%,#161a24 100%)",
  "radial-gradient(105% 84% at 28% 14%,#e8bda4 0%,rgba(232,189,164,0) 48%),linear-gradient(152deg,#b3735a 0%,#68453a 46%,#2b1f1b 100%)",
];

createApp({
  setup() {
    return () => h("div", { style: "min-height:100vh;background:var(--c-bg);padding-top:38px" }, [
      h(StatBand, { items: statItems, style: "margin-bottom:0" }),
      h("div", { style: "height:1px;background:var(--c-rule);margin:0 52px" }),
      h("div", { style: "padding:6px 52px 0;display:flex;align-items:baseline;gap:14px" }, [
        h("span", { style: "font-size:19px;font-weight:600;color:var(--c-fg);letter-spacing:-.2px" }, "2026"),
        h(GroupStat, { items: [{ value: 5, label: "个月" }, { value: 388, label: "张" }, { value: 78, label: "平均/月" }], dist: [40,62,48,70,96], distHighlight: 4 }),
      ]),
      h("div", { style: "padding:22px 52px 0;display:grid;grid-template-columns:repeat(4,1fr);gap:34px 30px" },
        albums.map((a, i) => h("div", { key: i }, [
          h("div", { style: `aspect-ratio:16/10;border-radius:5px;background:${grads[i]};box-shadow:var(--cover-inset),var(--cover-topline)` }),
          h("div", { style: "padding-top:11px;display:flex;align-items:baseline;justify-content:space-between;gap:10px" }, [
            h("b", { style: "font-size:13px;font-weight:560;color:var(--c-fg)" }, a.name),
            h("span", { style: "font-size:11px;color:var(--c-fg-3)" }, a.meta),
          ]),
        ]))),
    ]);
  },
}).mount("#app");
