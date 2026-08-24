<script setup lang="ts">
import { onMounted } from "vue";
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
onMounted(async () => {
  await auth.checkSession();
  if (auth.user) {
    store.fetchAlbums().catch(() => {});
  }
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
