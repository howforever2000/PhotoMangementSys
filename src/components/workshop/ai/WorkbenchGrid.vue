<script setup lang="ts">
/**
 * 工作台 · 选片网格（FEAT-180）
 * ===================================================================
 * 中栏的「专门选图排布窗口」：一次看全工作集，勾批量目标、点样片、移出工作集。
 * 单击 = 选中当前图（自选样片模式下兼作勾样片），双击 = 回单张对比。
 *
 * 和胶片条的分工：胶片条负责「在序列里移动」，网格负责「一次看完再挑」。
 * 两侧栏都可折叠，所以网格能吃到接近整窗的宽度。
 *
 * 纯展示 + 事件：勾选/样片语义由页面与 store 决定。
 */
import type { WbThumbItem } from "../../../types/workbench";

withDefaults(
  defineProps<{
    items: WbThumbItem[];
    currentPath: string;
    picking?: boolean;
  }>(),
  { picking: false },
);

const emit = defineEmits<{
  pick: [path: string];
  "toggle-select": [path: string];
  remove: [path: string];
  loupe: [];
}>();

function statusText(it: WbThumbItem): string {
  switch (it.status) {
    case "queued":
      return "排队";
    case "running":
      return `处理中 ${it.progress}%`;
    case "done":
      return "完成";
    case "failed":
      return "失败";
    default:
      return "待处理";
  }
}
</script>

<template>
  <div class="wg">
    <div class="wg-grid">
      <div
        v-for="it in items"
        :key="it.path"
        class="wg-tile"
        :class="{ on: it.path === currentPath, sample: it.sample }"
        :title="picking ? '点选/取消样片' : '单击选中，双击回对比'"
        @click="emit('pick', it.path)"
        @dblclick="emit('loupe')"
      >
        <div class="wg-thumb">
          <img v-if="it.src" :src="it.src" :alt="it.name" loading="lazy" draggable="false" />
          <span v-else class="wg-ph" />

          <button
            class="wg-check"
            :class="{ on: it.selected }"
            type="button"
            title="勾选为批量套用目标"
            @click.stop="emit('toggle-select', it.path)"
          >
            ✓
          </button>
          <button
            class="wg-del"
            type="button"
            title="从工作集移除（原图不动）"
            @click.stop="emit('remove', it.path)"
          >
            ✕
          </button>

          <span v-if="statusText(it)" class="wg-badge" :class="it.status">{{ statusText(it) }}</span>
          <span v-if="it.sample" class="wg-sample">样片</span>
          <div v-if="it.status === 'running' || it.status === 'queued'" class="wg-prog">
            <i :style="{ width: `${it.progress}%` }" />
          </div>
        </div>
        <div class="wg-name">{{ it.name }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.wg {
  height: 100%;
  min-height: 0;
  overflow-y: auto;
  padding-right: 2px;
  scrollbar-width: thin;
  scrollbar-color: rgba(255, 255, 255, 0.16) transparent;
}
.wg::-webkit-scrollbar {
  width: 6px;
}
.wg::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.14);
  border-radius: 3px;
}
.wg-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(148px, 1fr));
  gap: 12px;
  align-content: start;
}
.wg-tile {
  cursor: pointer;
  min-width: 0;
}
.wg-thumb {
  position: relative;
  aspect-ratio: 4 / 3;
  border-radius: var(--r-cover);
  overflow: hidden;
  background: var(--hover, rgba(120, 120, 130, 0.1));
  box-shadow: inset 0 0 0 1px var(--c-rule);
}
.wg-tile:hover .wg-thumb {
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 45%, transparent);
}
.wg-tile.on .wg-thumb {
  box-shadow: inset 0 0 0 2px var(--c-accent);
}
.wg-tile.sample .wg-thumb {
  box-shadow: inset 0 0 0 2px color-mix(in srgb, var(--c-accent) 60%, transparent);
}
.wg-thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.wg-ph {
  position: absolute;
  inset: 0;
  background: var(--hover, rgba(120, 120, 130, 0.1));
}

.wg-check,
.wg-del {
  position: absolute;
  top: 6px;
  width: 18px;
  height: 18px;
  padding: 0;
  border-radius: 5px;
  display: grid;
  place-items: center;
  font-size: 10px;
  line-height: 1;
  cursor: pointer;
}
.wg-check {
  left: 6px;
  border: 1px solid rgba(255, 255, 255, 0.5);
  background: rgba(10, 9, 13, 0.55);
  color: transparent;
}
.wg-check.on {
  background: var(--c-accent);
  border-color: var(--c-accent);
  color: #1a1608;
}
.wg-del {
  right: 6px;
  border: 1px solid rgba(255, 255, 255, 0.22);
  background: rgba(12, 10, 16, 0.66);
  color: #fff;
  opacity: 0;
  transition: opacity var(--dur-2) var(--ease-out);
}
.wg-tile:hover .wg-del,
.wg-del:focus-visible {
  opacity: 1;
}
.wg-del:hover {
  background: var(--c-danger, #c2504d);
  border-color: transparent;
}

.wg-badge {
  position: absolute;
  left: 6px;
  bottom: 6px;
  padding: 1px 7px;
  border-radius: var(--r-pill);
  font-size: 9.5px;
  background: rgba(10, 9, 13, 0.66);
  /* 压暗色角标：固定浅色字，浅色主题下也可读 */
  color: rgba(255, 255, 255, 0.78);
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.1);
}
.wg-badge.done {
  color: #9fe0a8;
}
.wg-badge.failed {
  color: #ff9a95;
}
.wg-badge.running,
.wg-badge.queued {
  color: #f3d08c;
}
.wg-sample {
  position: absolute;
  right: 6px;
  bottom: 6px;
  padding: 1px 6px;
  border-radius: 4px;
  font-size: 9px;
  font-weight: 700;
  color: #1a1608;
  background: var(--c-accent);
}
.wg-prog {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 3px;
  background: rgba(0, 0, 0, 0.4);
}
.wg-prog i {
  display: block;
  height: 100%;
  background: var(--c-accent);
}
.wg-name {
  margin-top: 5px;
  font-size: 10.5px;
  color: var(--fg-2);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
