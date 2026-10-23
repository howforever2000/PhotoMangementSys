<script setup lang="ts">
import { computed } from "vue";
import { useEditStore } from "../../../stores/edit";
import type { AdjustmentRecipe, CurvePoint } from "../../../types/edit";
import { cloneRecipe, readParam, writeParam } from "../../../types/edit";
import AdjustSlider from "./AdjustSlider.vue";
import CurveEditor from "./CurveEditor.vue";

/**
 * 右侧调整面板（FEAT-099）
 * ===================================================================
 * 布局完全由后端 `edit_schema` 驱动：分组顺序、滑块名称/范围/步长、
 * 数值显示格式、帮助文案全部来自 schema，前端**不含任何硬编码参数**。
 *
 * 好处是「加一个滑块」这件事的成本降到「只改 Rust 的 `schema.rs` 一处」，
 * 不可能出现「后端接受新范围、前端还按旧范围钳制」的错位。
 *
 * ## 折叠状态
 *
 * 用 `localStorage` 持久化（key 含图源路径的 hash，避免不同照片互相污染）。
 * 面板折叠与否是纯个人偏好，不该每次进编辑器都重排一遍。
 */
const store = useEditStore();

/** 分组折叠态（key = group key + 配方指纹）。 */
const collapsed = defineModel<Record<string, boolean>>("collapsed", { required: true });

const groups = computed(() => {
  const s = store.schema;
  if (!s) return [];
  return [...s.groups].sort((a, b) => a.order - b.order);
});

/** 某个分组是否处于「已修改」状态（组名旁显示一个小圆点）。 */
function groupDirty(g: { key: string; params: { key: string; neutral: number }[] }): boolean {
  return g.params.some((p) => readParam(store.recipe, p.key) !== p.neutral);
}

function isCollapsed(gkey: string): boolean {
  return collapsed.value[gkey] === true;
}

function toggle(gkey: string): void {
  collapsed.value = { ...collapsed.value, [gkey]: !isCollapsed(gkey) };
}

/* ---------------- 滑块事件 ---------------- */

/**
 * 滑块实时值（拖动中）。
 *
 * 只改 store 里的配方并触发低画质重渲，**不入撤销栈**（`commit=false`）。
 */
function onSliderInput(key: string, value: number): void {
  const next = cloneRecipe(store.recipe);
  writeParam(next, key, value);
  void store.applyRecipe(next, false);
}

/** 滑块提交（一次离散操作结束）→ 记一步撤销 + 高画质重渲。 */
function onSliderCommit(key: string, value: number): void {
  const next = cloneRecipe(store.recipe);
  writeParam(next, key, value);
  void store.applyRecipe(next, true);
}

function onSliderBegin(): void {
  store.beginInteraction();
}

/* ---------------- 曲线事件 ---------------- */

function curveOf(r: AdjustmentRecipe, key: string): CurvePoint[] {
  return key === "curveRgb" ? r.curveRgb.points : r.curveLuma.points;
}

function setCurve(r: AdjustmentRecipe, key: string, pts: CurvePoint[]): void {
  if (key === "curveRgb") r.curveRgb.points = pts;
  else r.curveLuma.points = pts;
}

function onCurveInput(key: string, pts: CurvePoint[]): void {
  const next = cloneRecipe(store.recipe);
  setCurve(next, key, pts);
  void store.applyRecipe(next, false);
}

function onCurveCommit(key: string, pts: CurvePoint[]): void {
  const next = cloneRecipe(store.recipe);
  setCurve(next, key, pts);
  void store.applyRecipe(next, true);
}

/* ---------------- 裁剪 ---------------- */

function onCropUpdate(c: AdjustmentRecipe["crop"]): void {
  const next = cloneRecipe(store.recipe);
  next.crop = c;
  void store.applyRecipe(next, false);
}

/** 供模板调用的入口（避免在模板里写多行逻辑）。 */
defineExpose({ onCropUpdate, onCropCommit });
function onCropCommit(c: AdjustmentRecipe["crop"]): void {
  const next = cloneRecipe(store.recipe);
  next.crop = c;
  void store.applyRecipe(next, true);
}
</script>

<template>
  <div class="edit-panel">
    <div v-if="!store.schema" class="ep-loading">参数表加载中…</div>

    <section v-for="g in groups" :key="g.key" class="ep-group">
      <header class="ep-head" @click="toggle(g.key)">
        <svg viewBox="0 0 16 16" class="ep-chev" :class="{ open: !isCollapsed(g.key) }" aria-hidden="true">
          <path
            d="M6 4l4 4-4 4"
            stroke="currentColor"
            stroke-width="2"
            fill="none"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
        <span class="ep-title">{{ g.label }}</span>
        <span v-if="groupDirty(g)" class="ep-dirty" title="该组已修改"></span>
      </header>

      <div v-show="!isCollapsed(g.key)" class="ep-body">
        <AdjustSlider
          v-for="p in g.params"
          :key="p.key"
          :spec="p"
          :model-value="readParam(store.recipe, p.key)"
          :busy="store.rendering"
          @begin="onSliderBegin"
          @update:model-value="(v: number) => onSliderInput(p.key, v)"
          @commit="(v: number) => onSliderCommit(p.key, v)"
        />

        <CurveEditor
          v-for="c in g.curves"
          :key="c.key"
          :spec="c"
          :points="curveOf(store.recipe, c.key)"
          :active="store.rendering"
          @begin="onSliderBegin"
          @update:points="(pts: CurvePoint[]) => onCurveInput(c.key, pts)"
          @commit:points="(pts: CurvePoint[]) => onCurveCommit(c.key, pts)"
        />

        <!-- 裁剪组：角度滑块 + 一个快捷入口；取景框交互在画布上。
             这里的「角度」滑块与 CropOverlay 的拉直线共用同一个字段，
             两条路径（拖线 / 拖滑块）互相可见，用户可以任选其一。 -->
        <template v-if="g.key === 'crop'">
          <p class="ep-note">
            在画布上拖动裁剪框取景；「拉直」模式下拖地平线，或直接调下面的角度。
          </p>
          <button type="button" class="ep-wide-btn" @click="store.cropping = !store.cropping">
            {{ store.cropping ? "完成裁剪" : "在画布上裁剪" }}
          </button>
          <button
            type="button"
            class="ep-wide-btn"
            style="margin-top: 6px"
            :disabled="!store.recipe.crop.enabled"
            @click="
              () => {
                const c = { ...store.recipe.crop, x: 0, y: 0, w: 1, h: 1, angle: 0, enabled: false };
                onCropCommit(c);
              }
            "
          >
            恢复整幅
          </button>
        </template>
      </div>
    </section>
  </div>
</template>

<style scoped>
.edit-panel {
  width: 320px;
  padding: 4px 10px 16px;
  overflow-y: auto;
  overflow-x: hidden;
  /* 细滚动条，避免在窄面板里占掉 15px 内容宽度 */
  scrollbar-width: thin;
}
.edit-panel::-webkit-scrollbar {
  width: 6px;
}
.edit-panel::-webkit-scrollbar-thumb {
  background: rgba(128, 128, 138, 0.35);
  border-radius: 3px;
}

.ep-loading {
  padding: 20px 4px;
  font-size: 12px;
  color: var(--pm-hint, var(--color-text-2));
  text-align: center;
}

.ep-group {
  border-bottom: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.16));
  padding-bottom: 4px;
}
.ep-group:last-child {
  border-bottom: none;
}

.ep-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 9px 2px 7px;
  cursor: pointer;
  user-select: none;
}
.ep-head:hover .ep-title {
  color: var(--color-text, #e6e9f0);
}
.ep-chev {
  width: 12px;
  height: 12px;
  color: var(--pm-hint, var(--color-text-2));
  transition: transform var(--dur-2);
  flex-shrink: 0;
}
.ep-chev.open {
  transform: rotate(90deg);
}
.ep-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--pm-label, var(--color-text-2));
  transition: color var(--dur-2);
}
.ep-dirty {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-primary, rgba(140, 175, 235, 0.9));
}

.ep-body {
  padding: 2px 0 8px;
}

.ep-note {
  margin: 4px 0 8px;
  font-size: 11px;
  line-height: 1.5;
  color: var(--pm-hint, var(--color-text-2));
}

.ep-wide-btn {
  width: 100%;
  padding: 6px 0;
  font-size: 12px;
  color: var(--color-text, #e6e9f0);
  background: var(--pm-btn-bg, rgba(128, 128, 138, 0.16));
  border: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.22));
  border-radius: 7px;
  cursor: pointer;
}
.ep-wide-btn:hover {
  background: var(--pm-btn-hover, rgba(128, 128, 138, 0.28));
}
</style>