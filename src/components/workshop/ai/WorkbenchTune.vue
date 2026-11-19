<script setup lang="ts">
/**
 * 工作台 · 单步微调（FEAT-180）
 * ===================================================================
 * 配方的每一步都能就地改参数，改完即时预览（本机算子，免费）。
 *
 * 为什么参数表不写在这里：字段名/值域/默认值全部来自 `op_catalog.json`
 * （`workbenchPlan.paramViews`），与后端 `TUNABLE_PARAMS` 逐键一致。
 * 旧实现手写了一张 7 项白名单，重构后与后端悄悄漂移，且面板本体丢失 ——
 * 这次把「面板默认存在」和「参数单一真相源」一起修掉。
 *
 * 默认折叠：卡片高度有限，懒人主路径不该被 10 根滑杆占满。
 * 抽完样片由页面把 `open` 置 true 一次（用户已经看到样片，接下来就是调）。
 *
 * FEAT-182：每个参数都能标「不处理」——数值按 0（= 不做），枚举没有中性值，
 * 标了就让整步不参与出片。语义落在 `workbenchPlan.effectiveTuneSteps`，
 * 这里只负责画开关与说明，不自己解释参数值。
 *
 * 纯展示 + 事件：不改 props、不碰 store、不发请求。
 */
import { computed, ref, watch } from "vue";

import { opById } from "../../../utils/toolCatalog";
import {
  applySlider,
  coerceEnum,
  effectiveTuneSteps,
  enumText,
  opLabel,
  paramViews,
  sliderOf,
  stepSummary,
  tuneOffKey,
  type TuneOff,
  type WbChainStep,
} from "../../../utils/workbenchPlan";

const props = withDefaults(
  defineProps<{
    /** 配方步骤 + 当前参数（catalog 值域） */
    steps: WbChainStep[];
    /** 配方默认参数（判改动 / 复位） */
    base: WbChainStep[];
    /** 单参数「不处理」：op.key → true */
    off?: TuneOff;
    /** 已改参数个数（store 口径，不含不处理） */
    changedCount?: number;
    /** 不处理参数个数 */
    skippedCount?: number;
    disabled?: boolean;
    /** 面板展开状态（v-model:open） */
    open?: boolean;
  }>(),
  { off: () => ({}), changedCount: 0, skippedCount: 0, disabled: false, open: false },
);

const emit = defineEmits<{
  "update:open": [value: boolean];
  param: [uid: string, key: string, value: number | string | boolean];
  /** 下拉选「不处理」= 整步不参与；选回真实枚举值 = 恢复 */
  off: [uid: string, key: string, off: boolean];
  "reset-step": [uid: string];
  "reset-all": [];
}>();

/** 下拉里的「不处理」哨兵值（不会与 LUT/风格的真实 id 撞车） */
const OFF = "__off";

/** 手风琴：一次只展开一步，面板不会长到视口外（按 uid，同名算子各展开各的） */
const openUid = ref<string | null>(null);

const baseMap = computed(() => new Map(props.base.map((s) => [s.uid, s.params])));

function isOff(uid: string, key: string): boolean {
  return Boolean(props.off[tuneOffKey(uid, key)]);
}

/** 这一步是否有参数被改过或被标为不处理（决定「复位」是否出现） */
function opChanged(uid: string): boolean {
  const step = props.steps.find((s) => s.uid === uid);
  if (!step) return false;
  const before = baseMap.value.get(uid);
  for (const [key, value] of Object.entries(step.params)) {
    if (isOff(uid, key)) return true;
    if (!before || before[key] !== value) return true;
  }
  return false;
}

/** 枚举参数被标不处理 → 整步不参与出片（与实际下发语义一致） */
function stepOff(step: WbChainStep): boolean {
  const spec = opById(step.op);
  return Object.entries(spec?.params ?? {}).some(
    ([key, p]) => p.type === "enum" && isOff(step.uid, key),
  );
}

/** 摘要按「实际下发值」算：数值不处理显示 0，枚举不处理显示不处理 */
function summaryOf(step: WbChainStep): string {
  const eff = effectiveTuneSteps([step], props.off)[0];
  return eff ? stepSummary(eff) : "不处理";
}

const changeText = computed(() =>
  [
    props.changedCount ? `已改 ${props.changedCount} 处` : "",
    props.skippedCount ? `跳过 ${props.skippedCount} 项` : "",
  ]
    .filter(Boolean)
    .join(" · "),
);

/** 展开面板且还没选中任何一步时，自动展开第一步有参数的步 —— 少点一次 */
watch(
  () => props.open,
  (v) => {
    if (!v || openUid.value) return;
    openUid.value = props.steps.find((s) => paramViews(s).length)?.uid ?? null;
  },
  { immediate: true },
);

/** 换配方后步骤集合会变：当前展开的步不存在时退回第一步 */
watch(
  () => props.steps.map((s) => s.uid),
  (uids) => {
    if (!openUid.value || !uids.includes(openUid.value)) openUid.value = uids[0] ?? null;
  },
);

function toggleOpen(uid: string): void {
  openUid.value = openUid.value === uid ? null : uid;
}

function togglePanel(): void {
  emit("update:open", !props.open);
}

function onRange(step: WbChainStep, key: string, e: Event): void {
  const slider = Number((e.target as HTMLInputElement).value);
  const raw = applySlider(step, key, slider)[key];
  emit("param", step.uid, key, raw);
}

function onSelect(step: WbChainStep, key: string, e: Event): void {
  const raw = (e.target as HTMLSelectElement).value;
  if (raw === OFF) {
    emit("off", step.uid, key, true);
    return;
  }
  // 从「不处理」选回任一真实值：先解除整步跳过，再写入该值
  if (isOff(step.uid, key)) emit("off", step.uid, key, false);
  emit("param", step.uid, key, coerceEnum(step.op, key, raw));
}
</script>

<template>
  <section class="wt" :class="{ open }">
    <button class="wt-head" type="button" :aria-expanded="open" @click="togglePanel">
      <span class="wt-chev" aria-hidden="true">{{ open ? "▾" : "▸" }}</span>
      <span class="wt-title">单步微调</span>
      <span class="wt-sub">{{ steps.length }} 步 · 本机免费</span>
      <span v-if="changeText" class="wt-changed">{{ changeText }}</span>
      <span v-else class="wt-sub wt-default">默认值</span>
    </button>

    <div v-if="open" class="wt-body">
      <p v-if="!steps.length" class="wt-tip">这个配方没有可调参数。</p>

      <ol v-else class="wt-list">
        <li
          v-for="(step, i) in steps"
          :key="step.uid"
          class="wt-step"
          :class="{ on: openUid === step.uid, dirty: opChanged(step.uid), off: stepOff(step) }"
        >
          <div class="wt-step-top">
            <span class="wt-n">{{ i + 1 }}</span>
            <button class="wt-name" type="button" :aria-expanded="openUid === step.uid" @click="toggleOpen(step.uid)">
              {{ opLabel(step.op) }}
              <span class="wt-sum">{{ summaryOf(step) }}</span>
              <span class="wt-chev2" aria-hidden="true">{{ openUid === step.uid ? "▾" : "▸" }}</span>
            </button>
            <button
              v-if="opChanged(step.uid)"
              class="wt-reset"
              type="button"
              title="这一步恢复配方默认值与「不处理」标记"
              :disabled="disabled"
              @click="emit('reset-step', step.uid)"
            >
              复位
            </button>
          </div>

          <div v-if="openUid === step.uid" class="wt-params">
            <div v-for="p in paramViews(step)" :key="p.key" class="wt-field">
              <div class="wt-field-h">
                <span>{{ p.label }}</span>
                <span class="wt-val">
                  {{
                    p.kind === "select"
                      ? isOff(step.uid, p.key)
                        ? "不处理"
                        : enumText(step, p.key)
                      : `${sliderOf(step, p.key)}%`
                  }}
                </span>
              </div>
              <select
                v-if="p.kind === 'select' && p.options.length"
                class="wt-select"
                :value="isOff(step.uid, p.key) ? OFF : p.value"
                :disabled="disabled"
                @change="onSelect(step, p.key, $event)"
              >
                <option v-for="o in p.options" :key="o.value" :value="o.value">{{ o.label }}</option>
                <!-- 下拉多一个「不处理」：枚举没有中性值，选了就是整步不参与 -->
                <option :value="OFF">不处理（跳过本步）</option>
              </select>
              <input
                v-else-if="p.kind === 'range'"
                class="wt-range"
                type="range"
                min="0"
                max="100"
                :value="sliderOf(step, p.key)"
                :disabled="disabled || stepOff(step)"
                @input="onRange(step, p.key, $event)"
              />
              <span v-else class="wt-val">{{ p.value }}</span>
            </div>
            <p v-if="stepOff(step)" class="wt-step-note">
              本步已选「不处理」；在下拉里选回任一真实值即可恢复。
            </p>
            <p v-if="!paramViews(step).length" class="wt-tip">这一步没有可调参数。</p>
          </div>
        </li>
      </ol>

      <div class="wt-foot">
        <span class="wt-foot-note">
          滑杆拉到 0 = 不处理；下拉选「不处理」= 整步跳过。只重算当前这张，免费。
        </span>
        <button
          v-if="changedCount || skippedCount"
          class="wt-reset-all"
          type="button"
          :disabled="disabled"
          @click="emit('reset-all')"
        >
          全部复位
        </button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.wt {
  margin-top: 10px;
  border-top: 1px solid var(--c-rule);
  padding-top: 8px;
}
.wt-head {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 100%;
  height: 26px;
  padding: 0 2px;
  border: none;
  background: none;
  color: var(--fg-2);
  font: inherit;
  font-size: var(--fs-body);
  text-align: left;
  cursor: pointer;
}
.wt-head:hover {
  color: var(--fg);
}
.wt-chev {
  flex: none;
  width: 10px;
  font-size: 9px;
}
.wt-title {
  flex: none;
  color: var(--fg);
  font-weight: 600;
}
.wt-sub {
  font-size: 10px;
  color: var(--fg-3, var(--fg-2));
  white-space: nowrap;
}
.wt-default {
  margin-left: auto;
}
.wt-changed {
  margin-left: auto;
  padding: 1px 7px;
  border-radius: 999px;
  font-size: 9.5px;
  color: var(--c-accent);
  border: 1px solid color-mix(in srgb, var(--c-accent) 55%, transparent);
  white-space: nowrap;
}

.wt-body {
  padding-top: 4px;
}
.wt-list {
  margin: 0;
  padding: 0;
  list-style: none;
}
.wt-step {
  padding: 3px 0;
  border-radius: 6px;
}
.wt-step.on {
  background: var(--hover);
}
.wt-step-top {
  display: flex;
  align-items: center;
  gap: 4px;
}
.wt-n {
  flex: none;
  width: 15px;
  height: 15px;
  border-radius: 50%;
  display: grid;
  place-items: center;
  font-size: 9px;
  color: var(--c-accent);
  background: color-mix(in srgb, var(--c-accent) 14%, transparent);
  font-variant-numeric: tabular-nums;
}
.wt-name {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 2px 0;
  border: none;
  background: none;
  color: var(--fg);
  font: inherit;
  font-size: var(--fs-body);
  text-align: left;
  cursor: pointer;
}
.wt-sum {
  margin-left: auto;
  font-size: 10px;
  color: var(--fg-3, var(--fg-2));
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 118px;
}
.wt-chev2 {
  flex: none;
  width: 9px;
  font-size: 9px;
  color: var(--fg-2);
}
.wt-step.dirty .wt-name {
  color: var(--c-accent);
}
.wt-reset {
  flex: none;
  height: 20px;
  padding: 0 7px;
  border-radius: 5px;
  border: 1px solid var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: 10px;
  cursor: pointer;
}
.wt-reset:hover:not(:disabled) {
  color: var(--c-accent);
  border-color: color-mix(in srgb, var(--c-accent) 45%, transparent);
}

.wt-params {
  padding: 2px 0 3px 19px;
}
.wt-field {
  margin: 7px 0;
}
.wt-field-h {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 6px;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  margin-bottom: 4px;
}
.wt-val {
  color: var(--c-accent);
  font-variant-numeric: tabular-nums;
}
.wt-step-note {
  margin: 4px 0 0;
  font-size: 9.5px;
  line-height: 1.5;
  color: var(--fg-3, var(--fg-2));
}
.wt-step.off {
  opacity: 0.72;
}
.wt-step.off .wt-range {
  opacity: 0.45;
}
.wt-range {
  width: 100%;
  accent-color: var(--c-accent);
}
.wt-select {
  width: 100%;
  height: 25px;
  padding: 0 7px;
  border-radius: 6px;
  border: 1px solid var(--c-rule);
  /* 跟随主题：深色卡片上是微亮底，浅色卡片上是微暗底 */
  background: color-mix(in srgb, var(--fg) 8%, transparent);
  color: var(--fg);
  font: inherit;
  font-size: var(--fs-meta);
}
.wt-tip {
  margin: 4px 0;
  font-size: 10.5px;
  color: var(--fg-3, var(--fg-2));
}
.wt-foot {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 6px;
  padding-top: 6px;
  border-top: 1px dashed var(--c-rule);
}
.wt-foot-note {
  flex: 1;
  font-size: 10px;
  line-height: 1.5;
  color: var(--fg-3, var(--fg-2));
}
.wt-reset-all {
  flex: none;
  height: 22px;
  padding: 0 8px;
  border-radius: 5px;
  border: 1px solid var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: 10px;
  cursor: pointer;
}
.wt-reset-all:hover:not(:disabled) {
  color: var(--c-accent);
}
.wt-reset:disabled,
.wt-reset-all:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
</style>
