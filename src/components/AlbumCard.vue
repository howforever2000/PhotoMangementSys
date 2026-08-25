<script setup lang="ts">
// 相册卡片组件 —— 从 AlbumList.vue 抽取
// 消除日期视图（未分类/月分组）与地点视图中 3 份重复的卡片模板
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Album } from "../types/album";
import { formatSize } from "../types/album";

defineProps<{
  album: Album;
  /** 是否处于勾选管理模式 */
  selectMode: boolean;
  /** 当前卡片是否被勾选 */
  selected: boolean;
  /** 是否显示地点徽标（地点排序视图用） */
  showLocation?: boolean;
}>();

const emit = defineEmits<{
  (e: "click"): void;
  (e: "contextmenu", ev: MouseEvent): void;
  (e: "toggle-select", ev: MouseEvent): void;
  (e: "open-path", ev: MouseEvent): void;
  /** FEAT-A：点击合并来源路径时触发（path 链接回系统文件夹） */
  (e: "open-source-path", payload: { id: number; path: string }): void;
}>();

/** 将本地文件路径转为前端可访问的 URL（Tauri asset 协议） */
function fileUrl(path: string | null): string {
  return path ? convertFileSrc(path) : "";
}

/** FEAT-A：合并来源路径点击 —— emit 携带 path，由父组件负责打开系统文件夹。 */
function emitSourcePath(src: { id: number; path: string }, ev: MouseEvent) {
  ev.stopPropagation();
  emit("open-source-path", { id: src.id, path: src.path });
}
</script>

<template>
  <article
    class="album-card"
    :class="{ 'card-selected': selected, 'card-manage': selectMode }"
    :tabindex="0"
    role="button"
    :aria-pressed="selectMode ? selected : undefined"
    @click="emit('click')"
    @contextmenu="(e) => emit('contextmenu', e)"
    @keydown.enter.space.prevent="emit('click')"
  >
    <!-- 勾选模式下显示勾选框 -->
    <div v-if="selectMode" class="card-checkbox" @click.stop="emit('toggle-select', $event)">
      <span :class="['checkmark', { checked: selected }]">✓</span>
    </div>

    <div class="card-cover">
      <img v-if="album.cover_path" :src="fileUrl(album.cover_path)" alt="封面" loading="lazy" />
      <div v-else class="cover-placeholder">📷</div>
    </div>

    <div class="card-body">
      <h3 class="card-name" :title="album.name">{{ album.name }}</h3>

      <!-- 地点徽标（仅地点排序视图显示） -->
      <p v-if="showLocation" class="location-tag" :class="{ 'no-loc': !album.location }">
        📍 {{ album.location || "未知地点" }}
      </p>

      <p v-if="album.description" class="card-desc">{{ album.description }}</p>

      <p class="card-path">
        <span
          class="path-link"
          :title="`在文件资源管理器中打开：${album.path}`"
          @click.stop="emit('open-path', $event)"
        >📁 {{ album.path }}</span>
      </p>

      <!-- FEAT-A：合并来源（FEAT-A）
        当该相册历史上被合并过，列出所有源相册路径。
        每条路径可点击 → 在系统文件管理器中打开源文件夹（已删除的源相册不会跳转；只展示信息） -->
      <div v-if="album.merged_sources && album.merged_sources.length" class="card-merged">
        <div class="merged-head">
          <span class="merged-label">由 {{ album.merged_sources.length }} 个相册合并而来</span>
        </div>
        <ul class="merged-list">
          <li v-for="src in album.merged_sources" :key="src.id" class="merged-item" :title="`${src.name}（已合并）`">
            <span
              class="merged-path"
              :title="`在文件管理器中打开源文件夹：${src.path}`"
              @click.stop="emitSourcePath({ id: src.id, path: src.path }, $event)"
            >📁 {{ src.path }}</span>
          </li>
        </ul>
      </div>

      <div class="card-stats">
        <span class="stat-item">🖼️ {{ album.photo_count }} 张</span>
        <span class="stat-item">💾 {{ formatSize(album.size_bytes) }}</span>
        <span v-if="album.shoot_time" class="stat-item">📅 {{ album.shoot_time }}</span>
      </div>
    </div>
  </article>
</template>

<style scoped>
.album-card {
  position: relative;
  background: var(--card-bg, #fff);
  color: var(--text, #333);
  border-radius: 12px;
  overflow: hidden;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.08);
  cursor: pointer;
  transition: transform 0.2s, box-shadow 0.2s;
}
.album-card:focus-visible {
  outline: none;
  box-shadow: 0 0 0 3px rgba(57, 108, 216, 0.45);
}

.album-card:hover {
  transform: translateY(-4px);
  box-shadow: 0 8px 20px rgba(0, 0, 0, 0.15);
}

.card-cover {
  height: 160px;
  background: var(--panel-bg, #f0f0f0);
  display: flex;
  align-items: center;
  justify-content: center;
}

.card-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.cover-placeholder {
  font-size: 48px;
  opacity: 0.4;
}

.card-body {
  padding: 14px;
}

.card-name {
  margin: 0 0 6px;
  font-size: 16px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.card-desc {
  margin: 0 0 8px;
  font-size: 13px;
  color: var(--sub-text, #666);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.card-path {
  margin: 0;
  font-size: 12px;
  color: var(--muted, #999);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.path-link {
  display: inline-block;
  max-width: 100%;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  vertical-align: bottom;
  color: #396cd8;
  cursor: pointer;
  text-decoration: underline;
  text-underline-offset: 2px;
}

.path-link:hover {
  color: #2f5cc2;
}

.card-stats {
  display: flex;
  gap: 14px;
  margin-top: 8px;
  padding-top: 8px;
  border-top: 1px solid var(--card-border, #f0f0f0);
}

.stat-item {
  font-size: 12px;
  color: var(--sub-text, #666);
}

/* 勾选模式 */
.card-checkbox {
  position: absolute;
  top: 10px;
  left: 10px;
  z-index: 5;
  display: flex;
  align-items: center;
  justify-content: center;
}

.checkmark {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  border: 2px solid var(--input-border, #ccc);
  background: var(--card-bg, rgba(255, 255, 255, 0.9));
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 13px;
  color: transparent;
  cursor: pointer;
  transition: all 0.2s;
}

.checkmark.checked {
  background: #396cd8;
  border-color: #396cd8;
  color: #fff;
}

.card-selected {
  border: 2px solid #396cd8;
  box-shadow: 0 0 0 2px rgba(57, 108, 216, 0.2);
}

/* 勾选管理模式：禁用悬停上浮，光标为默认 */
.card-manage {
  cursor: default;
}

.card-manage:hover {
  transform: none;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.08);
}

/* 地点徽标 */
.location-tag {
  display: inline-block;
  margin: 0 0 8px;
  padding: 2px 10px;
  font-size: 12px;
  color: #396cd8;
  background: #eef3ff;
  border-radius: 12px;
}

.location-tag.no-loc {
  color: var(--muted, #999);
  background: var(--panel-bg, #f0f0f0);
}

/* FEAT-A：合并来源列表 */
.card-merged {
  margin: 6px 0 4px;
  padding: 6px 8px;
  background: rgba(255, 153, 0, 0.06);
  border: 1px solid rgba(255, 153, 0, 0.18);
  border-radius: 6px;
}
.merged-head {
  font-size: 11px;
  font-weight: 600;
  color: #9a5b00;
  margin-bottom: 4px;
  display: flex;
  align-items: center;
  gap: 4px;
}
.merged-label {
  letter-spacing: 0.2px;
}
.merged-list {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-height: 88px;
  overflow-y: auto;
}
.merged-item {
  font-size: 11px;
  line-height: 1.45;
}
.merged-path {
  display: inline-block;
  max-width: 100%;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  vertical-align: bottom;
  color: #a06700;
  cursor: pointer;
  text-decoration: underline;
  text-underline-offset: 2px;
}
.merged-path:hover {
  color: #6f4800;
}
</style>
