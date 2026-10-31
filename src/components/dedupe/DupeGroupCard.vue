<script setup lang="ts">
/**
 * 一个重复组（FEAT-143）
 *
 * 只负责「把一组里的 N 张摊开 + 勾选/换保留」，不做任何 IO 与删除决策：
 * 缩略图由父组件统一排队取（`thumbQueue` 有全局在飞上限），删除由底部操作条统一下发。
 * 这样一组卡片就能独立预览、独立测（组件轻量 + 单一职责）。
 */
import { computed } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { DupGroup, DupItem } from "../../types/dedupe";
import { formatSize } from "../../types/album";

const props = defineProps<{
  group: DupGroup;
  /** 当前"保留"那张（用户可能已改选，故由父组件传入而不是读 group.keep_path） */
  keepPath: string;
  /** 路径 → 缩略图缓存路径（未取到时不渲染 img，显示占位） */
  thumbs: Record<string, string>;
  /** 待删除路径集合 */
  selected: Set<string>;
  /** 删除请求进行中：禁用勾选，防止状态错乱 */
  busy: boolean;
}>();

const emit = defineEmits<{
  (e: "toggle", path: string): void;
  (e: "setKeep", path: string): void;
  (e: "selectGroup", on: boolean): void;
}>();

const kindLabel = computed(() => (props.group.kind === "exact" ? "完全重复" : "画面相似"));

const deletableCount = computed(() => props.group.items.filter((i) => i.path !== props.keepPath).length);

const selectedInGroup = computed(
  () => props.group.items.filter((i) => props.selected.has(i.path)).length,
);

const allSelected = computed(() => deletableCount.value > 0 && selectedInGroup.value === deletableCount.value);

function thumbUrl(p: string): string {
  const t = props.thumbs[p];
  return t ? convertFileSrc(t) : "";
}

function dims(it: DupItem): string {
  return it.width && it.height ? `${it.width}×${it.height}` : "尺寸未知";
}

function fileName(p: string): string {
  return p.split(/[\\/]/).pop() || p;
}

/** 相似度展示：exact 组不必显示 1.000，读者会误以为"相似度算过" */
function simText(): string {
  return props.group.similarity.toFixed(3);
}
</script>

<template>
  <article class="dg" :class="`dg-${group.kind}`">
    <header class="dg-head">
      <span class="dg-kind">{{ kindLabel }}</span>
      <span class="dg-count">{{ group.items.length }} 张</span>
      <span v-if="group.kind === 'similar'" class="dg-sim" :title="'组内与『保留』那张的最低相似度'">
        相似度 ≥ {{ simText() }}
      </span>
      <span class="dg-gain">删其余可省 {{ formatSize(group.reclaimable_bytes) }}</span>
      <span class="dg-spacer" />
      <button
        class="dg-mini"
        type="button"
        :disabled="busy || deletableCount === 0"
        @click="emit('selectGroup', !allSelected)"
      >
        {{ allSelected ? "本组全不选" : "本组全选" }}
      </button>
    </header>

    <ul class="dg-strip">
      <li
        v-for="it in group.items"
        :key="it.path"
        class="dg-item"
        :class="{ 'is-keep': it.path === keepPath, 'is-checked': selected.has(it.path) && it.path !== keepPath }"
      >
        <div class="dg-thumb">
          <img v-if="thumbUrl(it.path)" :src="thumbUrl(it.path)" :alt="it.file_name" loading="lazy" />
          <span v-else class="dg-noimg">{{ it.file_name.slice(0, 1).toUpperCase() || "?" }}</span>
          <span v-if="it.path === keepPath" class="dg-badge">保留</span>
        </div>

        <label class="dg-pick">
          <input
            type="checkbox"
            :checked="selected.has(it.path)"
            :disabled="busy || it.path === keepPath"
            :aria-label="`勾选删除 ${it.file_name}`"
            @change="emit('toggle', it.path)"
          />
          <span class="dg-name" :title="it.path">{{ fileName(it.path) }}</span>
        </label>

        <div class="dg-meta">
          <span>{{ dims(it) }}</span>
          <span class="dg-dot">·</span>
          <span>{{ formatSize(it.size_bytes) }}</span>
        </div>

        <button
          v-if="it.path !== keepPath"
          class="dg-setkeep"
          type="button"
          :disabled="busy"
          @click="emit('setKeep', it.path)"
        >
          设为保留
        </button>
      </li>
    </ul>
  </article>
</template>

<style scoped>
.dg {
  border: 1px solid var(--c-rule);
  border-radius: 12px;
  background: color-mix(in srgb, var(--c-fg) 3%, transparent);
  padding: 10px 12px 12px;
}

.dg-head {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  font-size: var(--fs-sm, 12px);
  color: var(--c-fg-2);
  margin-bottom: 10px;
}

.dg-kind {
  padding: 2px 8px;
  border-radius: 999px;
  font-weight: var(--fw-em);
  font-size: 11.5px;
  color: #fff;
  background: color-mix(in srgb, var(--color-primary) 82%, #000);
}
.dg-similar .dg-kind {
  background: color-mix(in srgb, var(--color-warning, #c07a00) 82%, #000);
}

.dg-count {
  font-weight: var(--fw-em);
  color: var(--c-fg);
}
.dg-sim {
  font-variant-numeric: tabular-nums;
}
.dg-gain {
  color: var(--c-fg-2);
}
.dg-spacer {
  flex: 1;
}

.dg-mini {
  border: 1px solid var(--c-rule);
  background: transparent;
  color: var(--c-fg-2);
  border-radius: 7px;
  padding: 3px 9px;
  font-size: 12px;
  cursor: pointer;
}
.dg-mini:hover:not(:disabled) {
  color: var(--c-fg);
  background: color-mix(in srgb, var(--c-fg) 8%, transparent);
}
.dg-mini:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* 横向缩略图条：卡片内可横向滚动，组大时不把页面撑爆 */
.dg-strip {
  display: flex;
  gap: 10px;
  list-style: none;
  margin: 0;
  padding: 0 0 2px;
  overflow-x: auto;
}

.dg-item {
  flex: 0 0 148px;
  width: 148px;
  border: 1px solid transparent;
  border-radius: 10px;
  padding: 6px;
  transition: background var(--dur-2), border-color var(--dur-2);
}
.dg-item:hover {
  background: color-mix(in srgb, var(--c-fg) 5%, transparent);
}
.dg-item.is-keep {
  border-color: color-mix(in srgb, var(--color-primary) 55%, transparent);
  background: color-mix(in srgb, var(--color-primary) 8%, transparent);
}
.dg-item.is-checked .dg-thumb::after {
  content: "";
  position: absolute;
  inset: 0;
  border-radius: 8px;
  box-shadow: inset 0 0 0 2px color-mix(in srgb, var(--color-danger, #d13438) 70%, transparent);
}

.dg-thumb {
  position: relative;
  width: 100%;
  aspect-ratio: 1;
  border-radius: 8px;
  overflow: hidden;
  background: color-mix(in srgb, var(--c-fg) 8%, transparent);
  display: grid;
  place-items: center;
}
.dg-thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.dg-noimg {
  font-size: 26px;
  font-weight: 600;
  color: color-mix(in srgb, var(--c-fg) 40%, transparent);
}

.dg-badge {
  position: absolute;
  left: 6px;
  top: 6px;
  padding: 1px 7px;
  border-radius: 999px;
  font-size: 11px;
  font-weight: var(--fw-em);
  color: #fff;
  background: color-mix(in srgb, var(--color-primary) 88%, #000);
}

.dg-pick {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 7px;
  cursor: pointer;
  min-height: 24px;
}
.dg-pick input {
  width: 15px;
  height: 15px;
  accent-color: var(--color-danger, #d13438);
  cursor: pointer;
  flex: 0 0 auto;
}
.dg-pick input:disabled {
  cursor: not-allowed;
}

.dg-name {
  font-size: 12px;
  color: var(--c-fg);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dg-meta {
  display: flex;
  gap: 4px;
  font-size: 11.5px;
  color: var(--c-fg-2);
  font-variant-numeric: tabular-nums;
  padding-left: 21px;
}
.dg-dot {
  opacity: 0.6;
}

.dg-setkeep {
  margin: 6px 0 0 21px;
  border: 1px solid var(--c-rule);
  background: transparent;
  color: var(--c-fg-2);
  border-radius: 7px;
  padding: 2px 8px;
  font-size: 11.5px;
  cursor: pointer;
  opacity: 0;
  transition: opacity var(--dur-2), color var(--dur-2);
}
.dg-item:hover .dg-setkeep,
.dg-item:focus-within .dg-setkeep {
  opacity: 1;
}
.dg-setkeep:hover:not(:disabled) {
  color: var(--c-fg);
}
.dg-setkeep:disabled {
  cursor: not-allowed;
}

/* 触屏/无 hover：始终显示换保留入口，否则移动端无法操作 */
@media (hover: none) {
  .dg-setkeep {
    opacity: 1;
  }
}
</style>
