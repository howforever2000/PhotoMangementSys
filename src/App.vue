<script setup lang="ts">
import { onBeforeUnmount, onMounted } from "vue";
import { useRouter } from "vue-router";
import { useAlbumStore } from "./stores/album";
import { useAuthStore } from "./stores/auth";
import { useThemeStore } from "./stores/theme";
import ToastContainer from "./components/ToastContainer.vue";
import GlassFilters from "./components/ui/GlassFilters.vue";

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

/* FEAT-101：液态材质不再「追光」。
   旧实现挂了一个全局 pointermove 委托，把光标坐标写进每张卡片的 --mx/--my，
   再由 ::before 的游标亮斑 × 边缘环 mask 画出「光跟着鼠标走」的折射。三条劝退理由：
     ① 物理方向反了 —— 亮斑峰值落在卡片**内部**，那是光穿过玻璃，而不是被玻璃边缘折射；
     ② 全站每帧读一次 getBoundingClientRect 再写自定义属性，卡片被持续拉进样式重算；
     ③ 材质应该是卡片的静态属性，不该随鼠标位置变 —— 一条全局动效把「玻璃」变成「魔法布」。
   现在液态光学全部静态（固定主光源左上 135°），见 main.css 的 M3 段。 */

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
  // 弹窗 / mask / dialog 出现时也跳过（由页面级处理；.psd-mask = 性能设置/模型管理弹窗，.mb-mask = 回忆页月度浏览框）
  if (document.querySelector(".pm-modal, .pg-mask, .lb-overlay, .context-menu, .dialog-mask, .psd-mask, .mb-mask")) return;
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
});
onBeforeUnmount(() => {
  window.removeEventListener("keydown", onGlobalEsc);
});
</script>

<template>
  <div class="app-shell">
    <!--
      FEAT-108：液态玻璃的 SVG 滤镜定义。
      必须挂在根节点（而不是各个消费方）——
      CSS 的 url(#id) 是全文档作用域的，但被引用的 <svg> 必须在同一个
      document 里，且不能 display:none。放根节点 = 全应用一份，
      且不会随页面切换被销毁（否则切页后顶栏的折射会突然消失）。
    -->
    <GlassFilters />

    <div class="app-bg-base" :style="theme.layerBase"></div>
    <div v-if="theme.layerImage" class="app-bg-img" :style="theme.layerImage"></div>
    <!-- FEAT-094：背景图上的遮罩（上重下轻）——页头/面包屑等直掽压在壁纸亮部上的文字靠它达标 -->
    <div v-if="theme.layerScrim" class="app-bg-scrim" :style="theme.layerScrim"></div>
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

/* 全局背景层：底层（纯色/渐变）+ 可选图片层（透明度只淡化图片）+ 图片遮罩，位于所有页面之下 */
.app-bg-base,
.app-bg-img,
.app-bg-scrim {
  position: fixed;
  inset: 0;
  z-index: 0;
  pointer-events: none;
}

.app-bg-base {
  transition: background var(--dur-3) ease;
}

.app-bg-img {
  background-repeat: no-repeat;
  transition: opacity var(--dur-3) ease;
}

/* 内容层：在背景之上 */
.app-content {
  position: relative;
  z-index: 1;
  min-height: 100vh;
}
</style>
