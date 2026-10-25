/**
 * 人物卡探针（FEAT-113 ⑦）
 *
 * 挂真实 PersonGallery.vue，只桩掉 Tauri 命令。
 * 验证点：**名字是否被截断** —— 用多个长度不一的真实感名字压测，
 * 直接量「文字实际需要的高度」vs「卡片给的高度」，不靠肉眼判。
 */
import { createApp, h } from "vue";
import { createPinia } from "pinia";
import { createRouter, createWebHashHistory } from "vue-router";
import "../src/assets/tokens.css";
import "../src/assets/main.css";
import PersonGallery from "../src/components/PersonGallery.vue";

/* 名字长度覆盖：2 字、3 字、4 字、长中文、带括号长中文、超长无空格 */
const NAMES = [
  "妈妈",
  "爸爸",
  "外婆（成都老宅）",
  "2026 毕业合影中的未命名人物",
  "豆豆",
  "UnnamedPersonFromTrip2026",
  "爷爷奶奶合影",
  "小李",
  "表哥家的孩子",
  "外公",
  "同事聚会未命名人物",
  "猫",
];
const PERSONS = NAMES.map((name, i) => ({
  id: `P${String(i + 1).padStart(3, "0")}`,
  name,
  face_count: [128, 96, 77, 64, 52, 41, 33, 28, 21, 15, 9, 4][i],
  created_at: `2026-0${1 + (i % 9)}-1${i % 9}`,
  avatar_path: null,
}));

const stub = async (cmd: string) => {
  if (cmd === "list_persons") return PERSONS;
  if (cmd === "get_person_avatars_bulk") return {};
  if (cmd === "get_current_user") return null;
  return null;
};
(window as any).__TAURI_INTERNALS__ = {
  invoke: stub,
  transformCallback: (cb: unknown) => cb,
  convertFileSrc: (p: string) => p,
  metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
};
(window as any).__TAURI__ = {
  core: { invoke: stub, convertFileSrc: (p: string) => p },
  event: { listen: async () => () => {}, emit: async () => {} },
};

/* 头像用渐变占位，避免依赖真实文件 */
const css = document.createElement("style");
css.textContent = `
.person-avatar-fallback {
  background: linear-gradient(135deg, hsl(268 40% 52%), hsl(200 42% 40%)) !important;
}
`;
document.head.appendChild(css);

const router = createRouter({
  history: createWebHashHistory(),
  routes: [{ path: "/", component: PersonGallery }],
});

const app = createApp({ render: () => h(PersonGallery) });
app.use(createPinia());
app.use(router);
router.isReady().then(() => {
  app.mount("#app");
  setTimeout(() => {
    (window as any).__PROBE_READY__ = true;
  }, 700);
});
