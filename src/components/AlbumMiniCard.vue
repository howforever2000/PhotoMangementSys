<script setup lang="ts">
// 手动排序视图的迷你相册卡 —— 从 ManualSort.vue 抽取
// 消除顶级/二级/三级分组中 3 份重复的相册条目模板
import { computed } from "vue";
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
  /** 是否处于勾选管理模式（显示勾选角标） */
  selectMode?: boolean;
  /** 是否被勾选 */
  selected?: boolean;
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

/** FEAT-036：该相册是否已入库（如有已识别照片） */
const isScanned = computed(() => (props.album?.scanned_photo_count || 0) > 0);
</script>

<template>
  <div
    class="album-mini"
    :data-folder-id="props.folderId === null ? '' : String(props.folderId)"
    :data-album-index="index"
    :class="{ dragging, 'mini-selected': selected, 'mini-select-mode': selectMode }"
    @pointerdown="emit('pointerdown', $event)"
    @click="emit('click', $event)"
    @contextmenu="emit('contextmenu', $event)"
  >
    <!-- 勾选角标（管理模式） -->
    <span v-if="selectMode" class="mini-check" :class="{ checked: selected }">✓</span>
    <img v-if="album?.cover_path" :src="fileUrl(album.cover_path)" class="mini-cover" loading="lazy" />
    <div v-else class="mini-cover placeholder">📷</div>
    <!-- album 未加载/缺失时显示占位，避免空白卡片（改进自 7fd21dd） -->
    <span class="mini-name">{{ album?.name || "…" }}</span>
    <!-- FEAT-036：已入库 / 未入库 小圆点（封面右下角） -->
    <span
      class="mini-scan"
      :class="isScanned ? 'mini-scan-in' : 'mini-scan-out'"
      :title="isScanned ? '已入库' : '未入库'"
    ></span>
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
  background: var(--color-surface, #fff);
  border: 1px solid var(--color-border, #f0f0f0);
  transition: all 0.2s;
  color: var(--color-text);
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
  background: rgba(120, 130, 150, 0.15);
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 20px;
}

.mini-name {
  font-size: 12px;
  font-weight: 500;
  text-align: center;
  color: var(--color-text);
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

/* 勾选管理模式：卡片点击提示 */
.album-mini.mini-select-mode {
  cursor: pointer;
}

/* 被勾选的卡片 */
.album-mini.mini-selected {
  border-color: #396cd8;
  box-shadow: 0 0 0 2px rgba(57, 108, 216, 0.25);
}

/* 勾选角标 */
.mini-check {
  position: absolute;
  top: 4px;
  right: 4px;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  border: 1.5px solid var(--color-border, #bbb);
  background: var(--color-surface, #fff);
  font-size: 11px;
  color: transparent;
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 2;
  transition: all 0.15s;
}

.mini-check.checked {
  background: #396cd8;
  border-color: #396cd8;
  color: #fff;
}

/* 角标需相对卡片定位 */
.album-mini {
  position: relative;
}

/* FEAT-036：已入库 / 未入库 小圆点（封面右下角，避免与勾选角标冲突） */
.mini-scan {
  position: absolute;
  bottom: 26px;
  right: 12px;
  width: 10px;
  height: 10px;
  border-radius: 50%;
  box-shadow: 0 0 0 2px rgba(255, 255, 255, 0.9);
}
.mini-scan-in {
  background: #2f9e44;
}
.mini-scan-out {
  background: #b3b3b3;
}
</style>
