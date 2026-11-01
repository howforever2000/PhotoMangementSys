<script setup lang="ts">
/**
 * 一个重复组（FEAT-143）
 *
 * ## 一个文件夹一张卡（BUG-2026-1101-005）
 *
 * 后端给的粒度是「文件」。但同一个文件夹里常常躺着同一张照片的好几份（内容逐字节相同、
 * 文件名不同），按文件铺开就会出现「6 张一模一样的缩略图」，其中几张还与「保留」那张
 * 地址相同 —— 用户会以为同一张照片被重复列出，更怕点删除把想留的那张删掉。
 *
 * 所以展示与勾选单位都是 **文件夹**：一张卡 = 一个地址，卡上标「N 份」，并列出文件名。
 * 点击卡片勾选 = 删除该文件夹里的全部副本；保留所在文件夹的卡整张不可勾选
 * （`utils/dedupeGroups.deletablePathsOf` 在 store 里再兜一道，保证想留的那张删不到）。
 *
 * 组件本身不做任何 IO 与删除决策：缩略图由父组件按视口排队取，删除由底部操作条统一下发。
 */
import { computed } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { DupGroup } from "../../types/dedupe";
import { formatSize } from "../../types/album";
import { cellBytes, folderCells, type FolderCell } from "../../utils/dedupeGroups";

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
  (e: "toggleCell", dir: string, on: boolean): void;
  (e: "setKeep", path: string): void;
  (e: "selectGroup", on: boolean): void;
  /** 点击缩略图：交给父组件开灯箱（本组件不做任何 IO） */
  (e: "preview", path: string): void;
}>();

const kindLabel = computed(() => (props.group.kind === "exact" ? "完全重复" : "画面相似"));

/** 一个文件夹一张卡（保留文件夹排第一） */
const cells = computed<FolderCell[]>(() => folderCells(props.group, props.keepPath));

/** 本组可删的卡片数（保留文件夹那张不算） */
const deletableCells = computed(() => cells.value.filter((c) => !c.isKeepFolder));

const allSelected = computed(
  () => deletableCells.value.length > 0 && deletableCells.value.every((c) => cellSelected(c)),
);

/** 组内照片总数 / 文件夹数（给标题用：`6 张 · 2 个文件夹`） */
const photoCount = computed(() => cells.value.reduce((n, c) => n + c.items.length, 0));

function cellSelected(cell: FolderCell): boolean {
  if (cell.isKeepFolder) return false;
  const del = cell.items.filter((it) => it.path !== props.keepPath);
  return del.length > 0 && del.every((it) => props.selected.has(it.path));
}

function cellPartial(cell: FolderCell): boolean {
  if (cell.isKeepFolder) return false;
  const del = cell.items.filter((it) => it.path !== props.keepPath);
  const hit = del.filter((it) => props.selected.has(it.path)).length;
  return hit > 0 && hit < del.length;
}

function thumbUrl(cell: FolderCell): string {
  const t = props.thumbs[cell.items[0].path];
  return t ? convertFileSrc(t) : "";
}

function dims(cell: FolderCell): string {
  const it = cell.items[0];
  return it.width && it.height ? `${it.width}×${it.height}` : "尺寸未知";
}

/** 卡片主标题：几份相同照片 */
function cellTitle(cell: FolderCell): string {
  return cell.items.length > 1 ? `${cell.items.length} 份相同照片` : "1 份";
}

/** 文件名列表（证明它们是不同文件，不是同一张被重复列出） */
function namesOf(cell: FolderCell): string[] {
  return cell.items.map((it) => it.file_name);
}

function fileList(cell: FolderCell): string {
  return namesOf(cell).join("、");
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
      <span class="dg-count">{{ photoCount }} 张 · {{ cells.length }} 个文件夹</span>
      <span v-if="group.kind === 'similar'" class="dg-sim" :title="'组内与『保留』那张的最低相似度'">
        相似度 ≥ {{ simText() }}
      </span>
      <span class="dg-gain">删其余可省 {{ formatSize(group.reclaimable_bytes) }}</span>
      <span class="dg-spacer" />
      <button
        class="dg-mini"
        type="button"
        :disabled="busy || deletableCells.length === 0"
        @click="emit('selectGroup', !allSelected)"
      >
        {{ allSelected ? "本组全不选" : "本组全选" }}
      </button>
    </header>

    <ul class="dg-strip">
      <li
        v-for="cell in cells"
        :key="cell.dir"
        class="dg-item"
        :class="{ 'is-keep': cell.isKeepFolder, 'is-checked': cellSelected(cell) }"
      >
        <button
          class="dg-thumb"
          type="button"
          :title="`${cell.items[0].path}\n点击看大图`"
          :aria-label="`预览 ${cellTitle(cell)}`"
          @click="emit('preview', cell.items[0].path)"
        >
          <img
            v-if="thumbUrl(cell)"
            :src="thumbUrl(cell)"
            :alt="cellTitle(cell)"
            loading="lazy"
          />
          <span v-else class="dg-noimg">{{ (cell.items[0].file_name.slice(0, 1) || "?").toUpperCase() }}</span>
          <span v-if="cell.isKeepFolder" class="dg-badge">保留</span>
          <span v-else-if="cell.items.length > 1" class="dg-badge dg-badge-count">
            {{ cell.items.length }} 份
          </span>
          <span class="dg-zoom" aria-hidden="true">⤢</span>
        </button>

        <!-- 保留所在文件夹：整张不可勾选（硬保证：想留的那张永远删不到） -->
        <div v-if="cell.isKeepFolder" class="dg-pick dg-pick-locked" :title="`保留：${keepPath}`">
          <span class="dg-lock" aria-hidden="true">🔒</span>
          <span class="dg-name">{{ cellTitle(cell) }} · 保留</span>
        </div>
        <label v-else class="dg-pick">
          <input
            type="checkbox"
            :checked="cellSelected(cell)"
            :indeterminate="cellPartial(cell)"
            :disabled="busy"
            :aria-label="`删除该文件夹的 ${cell.items.length} 份相同照片`"
            @change="emit('toggleCell', cell.dir, !cellSelected(cell))"
          />
          <span class="dg-name" :title="`删掉这个文件夹里的 ${cell.items.length} 份`">
            {{ cellTitle(cell) }}
          </span>
        </label>

        <div class="dg-meta">
          <span>{{ dims(cell) }}</span>
          <span class="dg-dot">·</span>
          <span>{{ formatSize(cellBytes(cell)) }}</span>
        </div>

        <!-- 地址：完整显示（不再截断成 …\img\Pictures 这种认不出的样子） -->
        <p class="dg-addr" :title="cell.dir">{{ cell.dir || "地址未知" }}</p>

        <!-- 文件名：同一文件夹里的多份靠这几行证明是不同文件 -->
        <p class="dg-files" :title="fileList(cell)">
          <span v-for="name in namesOf(cell)" :key="name" class="dg-file-line">{{ name }}</span>
        </p>

        <button
          v-if="!cell.isKeepFolder"
          class="dg-setkeep"
          type="button"
          :disabled="busy"
          @click="emit('setKeep', cell.items[0].path)"
        >
          设为保留
        </button>
      </li>
    </ul>

    <p v-if="cells.some((c) => c.isKeepFolder && c.items.length > 1)" class="dg-note">
      保留文件夹里还有
      {{ cells.find((c) => c.isKeepFolder)!.items.length - 1 }} 份相同照片，本工具不动它们
    </p>
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
  flex: 0 0 168px;
  width: 168px;
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
  display: block;
  width: 100%;
  padding: 0;
  border: 0;
  aspect-ratio: 1;
  border-radius: 8px;
  overflow: hidden;
  background: color-mix(in srgb, var(--c-fg) 8%, transparent);
  cursor: zoom-in;
}
.dg-thumb:focus-visible {
  outline: 2px solid var(--color-primary);
  outline-offset: 2px;
}
.dg-thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
/* 可点击的提示：hover 才出现，不抢缩略图的注意力 */
.dg-zoom {
  position: absolute;
  right: 5px;
  bottom: 5px;
  width: 20px;
  height: 20px;
  display: grid;
  place-items: center;
  border-radius: 6px;
  font-size: 12px;
  color: #fff;
  background: color-mix(in srgb, #000 55%, transparent);
  opacity: 0;
  transition: opacity var(--dur-2);
}
.dg-item:hover .dg-zoom,
.dg-thumb:focus-visible .dg-zoom {
  opacity: 1;
}
.dg-noimg {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
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
.dg-badge-count {
  background: color-mix(in srgb, var(--color-danger, #d13438) 85%, #000);
}

.dg-pick {
  display: flex;
  align-items: flex-start;
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
  margin-top: 2px;
}
.dg-pick input:disabled {
  cursor: not-allowed;
}
.dg-pick-locked {
  cursor: default;
  color: var(--c-fg-2);
}
.dg-lock {
  font-size: 11px;
  line-height: 18px;
}

.dg-name {
  font-size: 12px;
  color: var(--c-fg);
  font-weight: var(--fw-em);
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

/* 地址行：完整路径按词断开换行，最多 2 行（悬停看全） */
.dg-addr {
  margin: 4px 0 0 21px;
  font-size: 11px;
  line-height: 1.45;
  color: var(--c-fg-2);
  word-break: break-all;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

/* 文件名：一行一个，最多 3 行（多份相同照片靠这里分辨） */
.dg-files {
  margin: 3px 0 0 21px;
  font-size: 10.5px;
  line-height: 1.5;
  color: var(--c-fg-2);
  word-break: break-all;
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.dg-file-line {
  display: block;
}

.dg-setkeep {
  margin: 5px 0 0 21px;
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

.dg-note {
  margin: 8px 0 0;
  font-size: 11.5px;
  line-height: 1.5;
  color: var(--c-fg-2);
}

/* 触屏/无 hover：始终显示换保留入口，否则移动端无法操作 */
@media (hover: none) {
  .dg-setkeep {
    opacity: 1;
  }
}
</style>
