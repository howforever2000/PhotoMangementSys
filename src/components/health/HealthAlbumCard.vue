<script setup lang="ts">
/**
 * 入库体检 · 单册卡片（展示组件，无 IO）
 *
 * 只做三件事：报事实（磁盘/入库/缩略图/向量/缓存）、画三条覆盖率、列出问题。
 * 结论由父组件通过 `utils/albumHealth.ts` 推导后传入（`level` / `issues` / `coverage`），
 * 这样本组件能脱离应用单独渲染，判定口径改阈值也不用碰这里。
 */
import { computed } from "vue";
import { formatSize } from "../../types/album";
import type { AlbumHealthRow } from "../../types/health";
import type { Coverage, HealthIssue, HealthLevel } from "../../utils/albumHealth";

const props = defineProps<{
  row: AlbumHealthRow;
  level: HealthLevel;
  issues: HealthIssue[];
  coverage: Coverage;
}>();

const emit = defineEmits<{ open: []; scan: [] }>();

const levelText = computed(
  () => ({ bad: "需处理", warn: "有缺口", ok: "正常" })[props.level],
);

/** 百分比读数：只在有分母时才有意义，空册显示 "—" */
function pct(v: number, hasDenominator: boolean): string {
  if (!hasDenominator) return "—";
  return `${Math.round(v * 100)}%`;
}

const hasDisk = computed(() => props.row.photo_count > 0);
const hasScanned = computed(() => props.row.scanned > 0);
</script>

<template>
  <article class="hc" :class="`hc-${level}`">
    <header class="hc-head">
      <div class="hc-ident">
        <span class="hc-badge" :class="`hb-${level}`">{{ levelText }}</span>
        <b class="hc-name" :title="row.name">{{ row.name }}</b>
        <span class="hc-path" :title="row.path">{{ row.path }}</span>
      </div>
      <div class="hc-acts">
        <button class="hc-mini" type="button" @click="emit('scan')">去扫描</button>
        <button class="hc-mini" type="button" @click="emit('open')">打开相册</button>
      </div>
    </header>

    <!-- 事实行：一眼看全五个数，单位按「张 / 体积」各自带上 -->
    <div class="hc-facts">
      <span>磁盘 <b>{{ row.photo_count }}</b> 张</span>
      <span>已入库 <b>{{ row.scanned }}</b> 张</span>
      <span>缩略图 <b>{{ row.thumbs }}</b> 张</span>
      <span>向量 <b>{{ row.embeddings }}</b> 张</span>
      <span>缓存 <b>{{ formatSize(row.thumb_bytes) }}</b></span>
    </div>

    <!-- 三条覆盖率：入库以磁盘为分母，缩略图/向量以「已入库」为分母 -->
    <div class="hc-cov">
      <div class="cov">
        <span class="cov-label">入库</span>
        <span class="cov-track"
          ><i :style="{ width: `${Math.round(coverage.scan * 100)}%` }"
        /></span>
        <span class="cov-num">{{ pct(coverage.scan, hasDisk) }}</span>
      </div>
      <div class="cov">
        <span class="cov-label">缩略图</span>
        <span class="cov-track"
          ><i :style="{ width: `${Math.round(coverage.thumb * 100)}%` }"
        /></span>
        <span class="cov-num">{{ pct(coverage.thumb, hasScanned) }}</span>
      </div>
      <div class="cov">
        <span class="cov-label">向量</span>
        <span class="cov-track"
          ><i :style="{ width: `${Math.round(coverage.embed * 100)}%` }"
        /></span>
        <span class="cov-num">{{ pct(coverage.embed, hasScanned) }}</span>
      </div>
    </div>

    <!-- 问题清单：标签说"是什么"，提示说"为什么算问题" -->
    <ul v-if="issues.length" class="hc-issues">
      <li v-for="i in issues" :key="i.key" :class="`ii-${i.level}`">
        <span class="ii-dot" aria-hidden="true" />
        <span class="ii-label">{{ i.label }}</span>
        <span class="ii-hint">{{ i.hint }}</span>
      </li>
    </ul>
    <p v-else class="hc-fine">这册的数据是齐的：入库 / 缩略图 / 向量都没缺口。</p>
  </article>
</template>

<style scoped>
.hc {
  display: flex;
  flex-direction: column;
  gap: 9px;
  padding: 12px 14px;
  border: 1px solid var(--c-rule);
  border-radius: 12px;
  background: color-mix(in srgb, var(--c-fg) 3%, transparent);
}
/* 左侧一条状态色带：扫一眼列表就能分出"要处理 / 有缺口 / 正常" */
.hc-bad {
  border-left: 3px solid color-mix(in srgb, var(--color-danger) 80%, transparent);
}
.hc-warn {
  border-left: 3px solid color-mix(in srgb, var(--color-warning, #c07a00) 80%, transparent);
}
.hc-ok {
  border-left: 3px solid color-mix(in srgb, var(--c-fg-2) 35%, transparent);
}

.hc-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
}
.hc-ident {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
  flex: 1 1 320px;
}
.hc-badge {
  flex-shrink: 0;
  font-size: 10.5px;
  line-height: 1;
  padding: 3px 6px;
  border-radius: 5px;
  border: 1px solid transparent;
}
.hb-bad {
  color: var(--color-danger);
  border-color: color-mix(in srgb, var(--color-danger) 45%, transparent);
  background: color-mix(in srgb, var(--color-danger) 10%, transparent);
}
.hb-warn {
  color: var(--color-warning-text, #c07a00);
  border-color: color-mix(in srgb, var(--color-warning, #c07a00) 45%, transparent);
  background: color-mix(in srgb, var(--color-warning, #c07a00) 10%, transparent);
}
.hb-ok {
  color: var(--c-fg-2);
  border-color: var(--c-rule);
}
.hc-name {
  font-size: 13.5px;
  color: var(--c-fg);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 40%;
}
.hc-path {
  font-size: 11.5px;
  color: var(--c-fg-2);
  font-family: "Consolas", monospace;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}

.hc-acts {
  display: flex;
  gap: 6px;
  flex-shrink: 0;
}
.hc-mini {
  border: 1px solid var(--c-rule);
  background: transparent;
  color: var(--c-fg-2);
  border-radius: 7px;
  padding: 4px 10px;
  font-size: 11.5px;
  cursor: pointer;
  min-height: 26px;
}
.hc-mini:hover {
  color: var(--c-fg);
  background: color-mix(in srgb, var(--c-fg) 8%, transparent);
}

.hc-facts {
  display: flex;
  gap: 16px;
  flex-wrap: wrap;
  font-size: 12px;
  color: var(--c-fg-2);
}
.hc-facts b {
  color: var(--c-fg);
  font-variant-numeric: tabular-nums;
  font-weight: var(--fw-em);
}

.hc-cov {
  display: flex;
  flex-direction: column;
  gap: 5px;
}
.cov {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 11.5px;
  color: var(--c-fg-2);
}
.cov-label {
  width: 44px;
  flex-shrink: 0;
}
.cov-track {
  flex: 1;
  height: 5px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--c-fg) 12%, transparent);
  overflow: hidden;
}
.cov-track i {
  display: block;
  height: 100%;
  border-radius: 999px;
  background: color-mix(in srgb, var(--c-fg-2) 70%, transparent);
  transition: width var(--dur-2);
}
.cov-num {
  width: 38px;
  text-align: right;
  font-variant-numeric: tabular-nums;
}

.hc-issues {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.hc-issues li {
  display: flex;
  align-items: baseline;
  gap: 7px;
  font-size: 12px;
  line-height: 1.55;
  flex-wrap: wrap;
}
.ii-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  flex-shrink: 0;
  transform: translateY(-2px);
}
.ii-bad .ii-dot {
  background: var(--color-danger);
}
.ii-warn .ii-dot {
  background: var(--color-warning, #c07a00);
}
.ii-bad .ii-label {
  color: var(--color-danger);
  font-weight: var(--fw-em);
}
.ii-warn .ii-label {
  color: var(--color-warning-text, #c07a00);
  font-weight: var(--fw-em);
}
.ii-hint {
  color: var(--c-fg-2);
}

.hc-fine {
  margin: 0;
  font-size: 12px;
  color: var(--c-fg-2);
}

@media (max-width: 720px) {
  .hc-name {
    max-width: 100%;
  }
  .hc-path {
    width: 100%;
  }
}
</style>
