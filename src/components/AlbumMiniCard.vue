<script setup lang="ts">
// 手动排序视图的迷你相册卡 —— 从 ManualSort.vue 抽取
// 消除顶级/二级/三级分组中 3 份重复的相册条目模板
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Album } from "../types/album";

const props = defineProps<{
  /** 相册 ID（用于 data-album-index 与拖拽） */
  albumId: number;
  /** 所属分组 ID（null 表示顶级游离，用于 data-folder-id） */
  folderId: number | null;
  /** 组内下标（用于拖拽插入位置计算） */
  index: number;
  /** 相册对象（可能尚未在 store 中加载，显示占位） */
  album: Album | null;
  /** 是否正在拖拽该卡片 */
  dragging: boolean;
}>();

const emit = defineEmits<{
  (e: "pointerdown", ev: PointerEvent): void;
  (e: "click", ev: MouseEvent): void;
  (e: "contextmenu", ev: MouseEvent): void;
}>();

/** 将本地文件路径转为前端可访问的 URL（Tauri asset 协议） */
function fileUrl(path: string | null): string {
  return path ? convertFileSrc(path) : "";
}
</script>

<template>
  <div
    class="album-mini"
    :data-folder-id="props.folderId === null ? '' : String(props.folderId)"
    :data-album-index="index"
    :class="{ dragging }"
    @pointerdown="emit('pointerdown', $event)"
    @click="emit('click', $event)"
    @contextmenu="emit('contextmenu', $event)"
  >
    <img v-if="album?.cover_path" :src="fileUrl(album.cover_path)" class="mini-cover" loading="lazy" />
    <div v-else class="mini-cover placeholder">📷</div>
    <!-- album 未加载/缺失时显示占位，避免空白卡片（改进自 7fd21dd） -->
    <span class="mini-name">{{ album?.name || "…" }}</span>
  </div>
</template>

<style scoped>
.album-mini {
  display: flex;
  flex-direction: column;
  align-items: center;
  width: 90px;
  padding: 8px;
  border-radius: 8px;
  cursor: grab;
  background: #fff;
  border: 1px solid #f0f0f0;
  transition: all 0.2s;
}

.album-mini:hover {
  border-color: #396cd8;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.1);
}

.mini-cover {
  width: 70px;
  height: 50px;
  object-fit: cover;
  border-radius: 6px;
  margin-bottom: 6px;
}

.mini-cover.placeholder {
  background: #f0f0f0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 20px;
}

.mini-name {
  font-size: 12px;
  text-align: center;
  color: #333;
  max-width: 80px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 拖拽中的相册 */
.album-mini.dragging {
  opacity: 0.4;
  border-color: #396cd8;
}
</style>
