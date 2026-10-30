// 临时验证夹具：渲染「全局概览」改版原型（方案 A / B），供 playwright 截图。
// 不参与构建，验证完即删。
import { createApp, h } from "vue";
import { createPinia } from "pinia";
import { createRouter, createMemoryHistory } from "vue-router";
import "../../../../src/assets/tokens.css";
import "../../../../src/assets/widget.css";
import "../../../../src/assets/main.css";
import "../../../../src/assets/pm-dialog.css";
import { useThemeStore } from "../../../../src/stores/theme";

const { default: Proto } = await import("./proto.vue");

const router = createRouter({
  history: createMemoryHistory(),
  routes: [{ path: "/:p(.*)*", component: { render: () => h("div") } }],
});

const pinia = createPinia();
const app = createApp({
  render: () => h(Proto, { variant: new URLSearchParams(location.search).get("v") ?? "a" }),
});
app.use(pinia);
useThemeStore(pinia);
app.use(router);
await router.push("/smart");
await router.isReady();
app.mount("#app");
