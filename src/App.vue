<script setup lang="ts">
import { onMounted } from "vue";
import { useAlbumStore } from "./stores/album";
import { useAuthStore } from "./stores/auth";

// 应用根组件：只负责渲染路由出口

// 启动流程：
// 1. 恢复登录会话（get_current_user）；未登录由路由守卫导向登录页
// 2. 已登录才触发相册预校验：后端对全部相册做文件数变更探测，
//    目录有变化的相册立即重扫更新 SQL 统计。用户进入相册列表/详情前，
//    数据已是最新（增删照片后无需等待，启动即校验）。
// 失败不阻塞界面——列表页加载时会再次校验。
const auth = useAuthStore();
const store = useAlbumStore();
onMounted(async () => {
  await auth.checkSession();
  if (auth.user) {
    store.fetchAlbums().catch(() => {});
  }
});
</script>

<template>
  <router-view />
</template>
