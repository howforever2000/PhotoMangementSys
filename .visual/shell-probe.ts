import { createApp, h } from "vue";
import { createRouter, createWebHashHistory } from "vue-router";
import { createPinia } from "pinia";
import "../src/assets/tokens.css";
import "../src/assets/main.css";
import AppHeader from "../src/components/ui/AppHeader.vue";
import ModuleTabs from "../src/components/ui/ModuleTabs.vue";
import StatBand from "../src/components/ui/StatBand.vue";

/**
 * FEAT-105 验收探针：只渲染三个新增/改写的 UI 件（页头 + 二级导航 + 数据带），
 * 用桩路由避免真实页面的 Tauri invoke 依赖。
 * 目的不是"看起来像"，而是量出真实渲染的排版与配色。
 */
const router = createRouter({
  history: createWebHashHistory(),
  routes: [{ path: "/:pathMatch(.*)*", component: { render: () => null } }],
});

const SMART_TABS = [
  { label: "人物", to: "/smart" },
  { label: "内容分类", to: "/smart?tab=category" },
  { label: "地点", to: "/smart?tab=location" },
  { label: "回忆", to: "/memories" },
  { label: "时间线", to: "/timeline" },
  { label: "智能搜索", to: "/search" },
  { label: "以图搜图", to: "/search?mode=image" },
];

const STATS = [
  { label: "张照片", value: 653, unit: "张" },
  { label: "占用存储", value: "18.4 GB" },
  { label: "相册", value: 12, unit: "个" },
  { label: "已入库", value: 615, unit: "张" },
  { label: "人物", value: 170, unit: "位" },
];

const app = createApp({
  setup() {
    return () =>
      h("div", { style: "min-height:100vh" }, [
        // 深色：用真实预设的三段渐变（与 theme.ts 下发的 gradFrom/Mid/To 一致），
        // 不能写纯色 —— 审稿页无 body 类时会命中浅色 --c-fg（深字），深底+深字=一片黑。
        // 补 theme-dark 类让 tokens.css 的深色 --c-* 生效（真实应用里这是 body 类）。
        h("div", { class: "theme-dark", style: "background:linear-gradient(165deg,#3b2b52,#271d37 55%,#120e1c)" }, [
          h(AppHeader, { title: "智慧相册", icon: "🧠", meta: "170 位人物 · 12 个相册" }),
          h(ModuleTabs, { tabs: SMART_TABS }),
          h(StatBand, { items: STATS, style: "margin:14px 0 18px" }),
        ]),
        // 浅色
        h("div", { style: "background:#ddd7cd;padding-bottom:8px" }, [
          h(AppHeader, { title: "扫描入库", icon: "📥", meta: "10 个相册已入库" }),
          h(ModuleTabs, {
            tabs: [
              { label: "全局扫描入库", to: "/scan" },
              { label: "按年·地点", key: "a", disabled: true, why: "功能待开发：扫描写库后即可用" },
              { label: "重复清理", key: "b", disabled: true, why: "功能待开发：扫描写库后即可用" },
              { label: "扫描分组工具", to: "/scan/test" },
            ],
          }),
          h(StatBand, {
            items: [
              { label: "相册", value: 12, unit: "个" },
              { label: "照片", value: 653, unit: "张" },
              { label: "已入库相册", value: 10, unit: "个" },
              { label: "已入库照片", value: 615, unit: "张" },
              { label: "全局扫描任务", value: "—", note: "尚未运行" },
            ],
            style: "margin:14px 0 18px",
          }),
        ]),
      ]);
  },
});

app.use(createPinia());
app.use(router);
app.mount("#app");
