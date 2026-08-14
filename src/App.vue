<script setup lang="ts">
import { onMounted } from "vue";
import { useAlbumStore } from "./stores/album";

// 应用根组件：只负责渲染路由出口

// 启动预校验：挂载时触发一次 get_albums，后端对全部相册做文件数变更探测，
// 目录有变化的相册立即重扫更新 SQL 统计。用户进入相册列表/详情前，
// 数据已是最新（增删照片后无需等待，启动即校验）。
// 失败不阻塞界面——列表页加载时会再次校验。
const store = useAlbumStore();
onMounted(() => {
  store.fetchAlbums().catch(() => {});
});
</script>

<template>
  <router-view />
</template>
