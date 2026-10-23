import { createApp, h, ref } from "vue";
import { createRouter, createWebHashHistory } from "vue-router";
import { createPinia } from "pinia";
import "../src/assets/tokens.css";
import "../src/assets/main.css";
import AppHeader from "../src/components/ui/AppHeader.vue";

// 用 hash history + 桩路由，避免真实页面的 Tauri invoke 依赖
const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/:pathMatch(.*)*", component: { render: () => null } },
  ],
});

const apps = createApp({
  setup() {
    return () =>
      h("div", { style: "min-height:100vh;background:var(--c-bg)" }, [
        h(AppHeader, { title: "相册" }),
        h("div", { style: "padding:28px 52px;color:var(--c-fg-2);font-size:13px" },
          "页面内容从顶栏下方开始，顶栏吸顶时不会遮住它。".repeat(3)),
        h("div", { style: "padding:0 52px;display:flex;gap:16px" }, [
          h(AppHeader, { title: "时间线", compact: true }),
        ]),
      ]);
  },
});
apps.use(createPinia());
apps.use(router);
apps.mount("#app");
