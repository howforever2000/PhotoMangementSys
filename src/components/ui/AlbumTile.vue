<script setup lang="ts">
/**
 * AlbumTile —— 相册瓦片（FEAT-103 · P1）
 *
 * 与现有 AlbumCard（components/AlbumCard.vue）的关系：
 *   不是替代，是**并存**。AlbumCard 承载了大量业务逻辑（多选/右键菜单/
 *   拖拽排序/封面来源/归档角标/进度条…，见其 200+ 行脚本）。
 *   AlbumTile 只负责「封面 + 两行字」的视觉，P2 接入时由 AlbumList 自己
 *   决定用哪个 —— 这样改造可以逐页推进，不必一次动 2883 行的组件。
 *
 * 关键设计决定：**没有卡片容器**。
 *   深色背景下，卡片边框是一圈 1px 灰线。8~10 张卡叠在一起时，这些线
 *   彼此平行、等距、无层级差异，视觉上会形成"网格噪声"——用户看到的是
 *   "一堆框"，而不是"一堆照片"。裸封面时，照片自己的边缘就是边界，
 *   而且每张照片的明暗不同，天然产生分隔。
 *
 * 唯一保留的"框"是 1px 内描边（--cover-inset）：它不是装饰，
 *   是防止封面亮部与页面背景糊在一起的必要边界。顶部再加一道更亮的
 *   短线（--cover-topline），模拟"被光照到的纸边"——这是唯一的光学暗示。
 */
import { computed, ref } from "vue";

const props = withDefaults(
  defineProps<{
    /** 封面图 URL（已由父组件 convertFileSrc 处理） */
    cover?: string | null;
    /** 相册名 */
    name: string;
    /** 右侧元信息（如 "108 · 人物 42"） */
    meta?: string;
    /** 左上角标（如 "已扫描"） */
    badge?: string;
    /** 封面宽高比，默认 16/10（比 4/3 更宽的画幅，横向空间利用率更高） */
    ratio?: string;
  }>(),
  { cover: null, meta: "", badge: "", ratio: "16 / 10" },
);

const emit = defineEmits<{
  (e: "click", ev: MouseEvent): void;
  (e: "contextmenu", ev: MouseEvent): void;
}>();

/** 封面加载失败：显示占位而不是碎图图标。
 *  相册封面文件被移动/删除是常见情况（BUG-2026-1021-005 就是这类），
 *  这里只负责"不显示碎图"，不在这里弹错误 —— 上层有统一的失败提示。 */
const broken = ref(false);
const showCover = computed(() => !!props.cover && !broken.value);
</script>

<template>
  <article
    class="u-tile"
    role="button"
    tabindex="0"
    @click="emit('click', $event)"
    @contextmenu.prevent="emit('contextmenu', $event)"
    @keydown.enter.space.prevent="emit('click', $event as unknown as MouseEvent)"
  >
    <div class="u-cover" :style="{ aspectRatio: ratio }">
      <img
        v-if="showCover"
        :src="cover ?? undefined"
        alt=""
        loading="lazy"
        decoding="async"
        @error="broken = true"
      />
      <span v-else class="u-cover-ph" aria-hidden="true">📷</span>
      <span v-if="badge" class="u-badge">{{ badge }}</span>
    </div>

    <div class="u-cap">
      <b class="u-name">{{ name }}</b>
      <span v-if="meta" class="u-meta">{{ meta }}</span>
    </div>
  </article>
</template>

<style scoped>
.u-tile {
  display: flex;
  flex-direction: column;
  cursor: pointer;
  /* 悬停只改亮度与描边，不做位移/加阴影：
     密集网格里任何位移都会让邻格"抖"，而阴影会把裸封面重新变回卡片。 */
  transition:
    filter var(--dur-2) var(--ease-out),
    transform var(--dur-1) var(--ease-out);
}

.u-cover {
  position: relative;
  width: 100%;
  border-radius: var(--r-cover);
  overflow: hidden;
  background: rgba(255, 255, 255, 0.04);
  box-shadow: var(--cover-inset), var(--cover-topline);
  transition: box-shadow var(--dur-2) var(--ease-out);
}

.u-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.u-cover-ph {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  font-size: 22px;
  opacity: 0.32;
}

.u-badge {
  position: absolute;
  top: 9px;
  left: 9px;
  font-size: 9px;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  font-weight: var(--fw-strong);
  color: rgba(255, 255, 255, 0.86);
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.5);
}

.u-cap {
  padding-top: 11px;
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
}

.u-name {
  font-size: var(--fs-title);
  font-weight: var(--fw-em);
  letter-spacing: var(--ls-title);
  color: var(--c-fg);
  /* 相册名可能很长：单行截断而不是换行，避免同一行的两个瓦片图注高度不一 */
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}

.u-meta {
  font-size: var(--fs-meta);
  /* 图注右侧的统计（108 · 人物 42）是要读的信息，不是装饰 —— 用 fg-2 */
  color: var(--c-fg-2);
  letter-spacing: var(--ls-meta);
  white-space: nowrap;
  flex-shrink: 0;
  font-variant-numeric: tabular-nums;
}

/* 悬停 */
.u-tile:hover .u-cover {
  box-shadow:
    inset 0 0 0 1px rgba(255, 255, 255, 0.16),
    var(--cover-topline);
}
.u-tile:hover {
  filter: brightness(1.06);
}

/* 按压（全局 tokens.css 里也有 :active 基线，这里补上 transition-duration
   的收短，让按下比抬起更快 —— 这是"跟手"的关键细节） */
.u-tile:active {
  transform: scale(0.985);
  transition-duration: var(--dur-1);
}

/* 焦点：极简版面里默认 outline 会很难看，但不能没有（键盘可达性）。
   用 1.5px 强调色 + 2px 偏移，落在封面上而不是图注上。 */
.u-tile:focus-visible {
  outline: none;
}
.u-tile:focus-visible .u-cover {
  box-shadow:
    inset 0 0 0 1px rgba(255, 255, 255, 0.16),
    0 0 0 1.5px var(--c-accent),
    0 0 0 3.5px transparent;
}

/* 浅色：占位底与图注灰都要换成暗色系 */
body:not(.theme-dark) .u-cover {
  background: rgba(23, 22, 26, 0.05);
}
body:not(.theme-dark) .u-badge {
  color: rgba(255, 255, 255, 0.94);
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.42);
}
</style>
