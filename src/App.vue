<script setup lang="ts">
import { onBeforeUnmount, onMounted } from "vue";
import { useRouter } from "vue-router";
import { useAlbumStore } from "./stores/album";
import { useAuthStore } from "./stores/auth";
import { useThemeStore } from "./stores/theme";
import ToastContainer from "./components/ToastContainer.vue";

// 应用根组件：渲染全局背景层 + 路由出口 + 全局 Toast 容器
// 启动流程：
// 1. 恢复登录会话（get_current_user）；未登录由路由守卫导向登录页
// 2. 已登录才触发相册预校验：后端对全部相册做文件数变更探测，
//    目录有变化的相册立即重扫更新 SQL 统计。用户进入相册列表/详情前，
//    数据已是最新（增删照片后无需等待，启动即校验）。
// 失败不阻塞界面——列表页加载时会再次校验。
const auth = useAuthStore();
const store = useAlbumStore();
const theme = useThemeStore();
const router = useRouter();

/**
 * 液态材质追光（FEAT-086 表面光学）：一次事件委托，全站卡片生效
 *  —— 把光标在卡片内的坐标写入 --mx/--my，::after 追光层只动 opacity，不重排。
 *  类路径（.glass-surface/.glass-card）与内联路径（cardStyle + 补挂 .glass-card）都命中。
 *  rAF 合帧：pointermove 每帧最多写一次，避免高频 style 写入。
 */
let lightRaf = 0;
let lightEv: PointerEvent | null = null;
function onPointerLight(e: PointerEvent) {
  // 磨砂材质下 ::after 追光层不存在，直接短路，零写入
  if (!document.body.classList.contains("mat-liquid")) return;
  lightEv = e;
  if (lightRaf) return;
  lightRaf = requestAnimationFrame(() => {
    lightRaf = 0;
    const ev = lightEv;
    lightEv = null;
    if (!ev) return;
    const card = (ev.target as HTMLElement | null)?.closest?.(".glass-card, .glass-surface") as HTMLElement | null;
    if (!card) return;
    const r = card.getBoundingClientRect();
    card.style.setProperty("--mx", `${ev.clientX - r.left}px`);
    card.style.setProperty("--my", `${ev.clientY - r.top}px`);
  });
}

/**
 * FEAT-ESC：全局 ESC 行为
 *  - 优先级最低：当页面内有弹窗/选模式时由各页面拦截，这里不接管
 *  - 路由返回上一级：仅在非弹窗/非选模式时生效，且 history 深度>1
 *  - 主页（/home）或直接进入的页面：ESC 不做任何事（避免意外退出）
 *
 *  实现：通过 event.defaultPrevented 识别是否有页面级 onKey 拦截
 *        (各页面的 onKey 都会 e.preventDefault 关闭弹窗 / 退出选模式)
 *        若未被拦截，且当前不在 /home / /login 公共页 → 走 router.back()
 */
function onGlobalEsc(e: KeyboardEvent) {
  if (e.key !== "Escape") return;
  // 公开页（登录 / 注册 / 忘记密码）一律不响应
  const path = router.currentRoute.value.path;
  if (path === "/login" || path === "/register" || path === "/forgot-password") return;
  // 页面级 handler 拦截过了就不再处理
  if (e.defaultPrevented) return;
  // 在主页时不响应（避免误退出）
  if (path === "/home" || path === "/") return;
  // 输入框/可编辑元素不响应（避免丢失内容）
  const target = e.target as HTMLElement | null;
  const tag = target?.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable) return;
  // 弹窗 / mask / dialog 出现时也跳过（由页面级处理；.psd-mask = 性能设置/模型管理弹窗）
  if (document.querySelector(".pm-modal, .pg-mask, .lb-overlay, .context-menu, .dialog-mask, .psd-mask")) return;
  // 路由返回上一级
  if (window.history.length > 1) {
    e.preventDefault();
    router.back();
  }
}

onMounted(async () => {
  await auth.checkSession();
  if (auth.user) {
    store.fetchAlbums().catch(() => {});
  }
  window.addEventListener("keydown", onGlobalEsc);
  // 液态追光：passive 监听，只写自定义属性（磨砂材质下 ::after 不命中，零视觉开销）
  window.addEventListener("pointermove", onPointerLight, { passive: true });
});
onBeforeUnmount(() => {
  window.removeEventListener("keydown", onGlobalEsc);
  window.removeEventListener("pointermove", onPointerLight);
  if (lightRaf) cancelAnimationFrame(lightRaf);
});
</script>

<template>
  <div class="app-shell">
    <div class="app-bg-base" :style="theme.layerBase"></div>
    <div v-if="theme.layerImage" class="app-bg-img" :style="theme.layerImage"></div>
    <div class="app-content">
      <router-view />
    </div>
    <ToastContainer />
  </div>
</template>

<style>
.app-shell {
  position: relative;
  min-height: 100vh;
}

/* 全局背景层：底层（纯色/渐变）+ 可选图片层（透明度只淡化图片），位于所有页面之下 */
.app-bg-base,
.app-bg-img {
  position: fixed;
  inset: 0;
  z-index: 0;
  pointer-events: none;
}

.app-bg-base {
  transition: background 0.3s ease;
}

.app-bg-img {
  background-repeat: no-repeat;
  transition: opacity 0.3s ease;
}

/* 内容层：在背景之上 */
.app-content {
  position: relative;
  z-index: 1;
  min-height: 100vh;
}
</style>
