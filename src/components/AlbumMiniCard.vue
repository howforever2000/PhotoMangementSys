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
/* 卡片宽度由宿主（ManualSort）的 --mini-card-w 变量驱动（Ctrl+滚轮缩放），
   未设置时回落 90px 保持历史默认；封面/名称按宽度等比反算，宽高比不漂移。 */
.album-mini {
  display: flex;
  flex-direction: column;
  align-items: center;
  width: var(--mini-card-w, 90px);
  padding: 8px;
  border-radius: 8px;
  cursor: grab;
  /* 材质统一（FEAT-086 管线）：此前只有 var(--color-surface) 填充，
     切材质时外观不变。现在填充/描边/模糊/壁厚阴影读主题下发令牌，
     液态自动带模糊与湿边高光（main.css 的 body.mat-liquid .album-mini 补齐），
     普通自动回落实心。 */
  background-color: var(--glass-bg, var(--color-surface, #fff));
  border: 1px solid var(--glass-border, var(--color-border, #f0f0f0));
  -webkit-backdrop-filter: blur(var(--glass-blur, 0px)) saturate(var(--glass-saturate, 1))
    brightness(var(--glass-brightness, 1)) contrast(var(--glass-contrast, 1));
  backdrop-filter: blur(var(--glass-blur, 0px)) saturate(var(--glass-saturate, 1))
    brightness(var(--glass-brightness, 1)) contrast(var(--glass-contrast, 1));
  box-shadow: var(--shadow-1);
  transition: all var(--dur-2);
  color: var(--color-text);
}
/* 普通材质：与 body.mat-plain .glass-surface 同一口径 —— 关掉模糊与明度位移 */
body.mat-plain .album-mini {
  -webkit-backdrop-filter: none;
  backdrop-filter: none;
}

.album-mini:hover {
  border-color: var(--color-primary);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.1);
}

.mini-cover {
  width: calc(var(--mini-card-w, 90px) - 20px);
  height: calc((var(--mini-card-w, 90px) - 20px) * 50 / 70);
  object-fit: cover;
  border-radius: 6px;
  margin-bottom: 6px;
}

.mini-cover.placeholder {
  background: rgba(120, 130, 150, 0.15);
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: calc(var(--mini-card-w, 90px) * 0.22);
}

.mini-name {
  font-size: calc(var(--mini-card-w, 90px) * 0.133);
  font-weight: 500;
  text-align: center;
  color: var(--color-text);
  max-width: calc(var(--mini-card-w, 90px) - 10px);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 拖拽中的相册 */
.album-mini.dragging {
  opacity: 0.4;
  border-color: var(--color-primary);
}

/* 勾选管理模式：卡片点击提示 */
.album-mini.mini-select-mode {
  cursor: pointer;
}

/* 被勾选的卡片 */
.album-mini.mini-selected {
  border-color: var(--color-primary);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--color-primary) 25%, transparent);
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
  transition: all var(--dur-2);
}

.mini-check.checked {
  background: var(--color-primary);
  border-color: var(--color-primary);
  color: var(--color-on-primary);
}

/* 角标需相对卡片定位 */
.album-mini {
  position: relative;
}

/* FEAT-036：已入库 / 未入库 小圆点（封面右下角，避免与勾选角标冲突）；
   位置随卡片宽度等比缩放（90px 时即历史默认 26/12px） */
.mini-scan {
  position: absolute;
  bottom: calc(var(--mini-card-w, 90px) * 0.29);
  right: calc(var(--mini-card-w, 90px) * 0.13);
  width: 10px;
  height: 10px;
  border-radius: 50%;
  box-shadow: 0 0 0 2px rgba(255, 255, 255, 0.9);
}
.mini-scan-in {
  background: var(--color-ok-text);
}
.mini-scan-out {
  background: #b3b3b3;
}
</style>
