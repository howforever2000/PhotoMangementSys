<script setup lang="ts">
/**
 * 全选复选框（三态）—— 修 BUG-2026-1007-002
 *
 * 需求原话：「全选照片后点击全选不能取消全选，全选那个按钮默认应该是空圈圈，全选后才是打勾」。
 * 修复前的三种实现都不满足：
 * 1. `CategoryGallery` / `LocationGallery` / `PersonGallery` 用的是
 *    **「全选」按钮 + 独立的「取消全选」按钮** —— 没有「再点一次取消」的语义；
 * 2. `PhotoGrid` 用原生 `<input type=checkbox>`，只有 2 态（没有半选横杠），
 *    且外观取决于系统主题，不符合「空圈圈 / 打勾」的明确要求；
 * 3. `PhotoGrid` 的 `Ctrl+A` 与复选框两套状态不联动。
 *
 * 现在统一为一个组件：空圈 → 打勾（全部选中）→ 再点取消；部分选中显示横杠（indeterminate）。
 * 点击逻辑集中在这里，四个画廊共用，谁也不会再各写一遍。
 */
import { computed } from "vue";

const props = withDefaults(
  defineProps<{
    /** 当前已选数量 */
    selectedCount: number;
    /** 可选中的总数（当前视图内的可选项） */
    totalCount: number;
    /** 禁用（无可选项时） */
    disabled?: boolean;
    /** 文案，默认「全选」 */
    label?: string;
  }>(),
  { disabled: false, label: "全选" },
);

const emit = defineEmits<{ (e: "toggle", selectAll: boolean): void }>();

/** 全部选中（总数 > 0 且一个不落） */
const allChecked = computed(() => props.totalCount > 0 && props.selectedCount >= props.totalCount);
/** 部分选中：大于 0 但没全选 —— 显示横杠而不是打勾（Windows 资源管理器同款语义） */
const indeterminate = computed(
  () => props.selectedCount > 0 && props.selectedCount < props.totalCount,
);
/** 无可选项时禁用（避免「全选 0 张」这种无意义操作） */
const isDisabled = computed(() => props.disabled || props.totalCount <= 0);

function onClick() {
  if (isDisabled.value) return;
  // 复选框语义：已是全选 → 本次点击表示「取消全选」
  emit("toggle", !allChecked.value);
}
</script>

<template>
  <button
    type="button"
    class="sac"
    :class="{ checked: allChecked, partial: indeterminate }"
    :disabled="isDisabled"
    :aria-checked="allChecked ? 'true' : indeterminate ? 'mixed' : 'false'"
    role="checkbox"
    :title="
      allChecked
        ? `已选中全部 ${totalCount} 张，点击取消全选`
        : indeterminate
          ? `已选中 ${selectedCount} / ${totalCount} 张，点击全选`
          : `选中全部 ${totalCount} 张`
    "
    @click.stop="onClick"
  >
    <!-- 三态指示：空圈 / 横杠 / 打勾，用 CSS 画而非原生 checkbox，避免系统主题差异 -->
    <span class="sac-box" aria-hidden="true">
      <span v-if="allChecked" class="sac-check">✓</span>
      <span v-else-if="indeterminate" class="sac-dash">−</span>
    </span>
    <span class="sac-label">{{ label }}</span>
    <span v-if="totalCount > 0" class="sac-count">{{ selectedCount }}/{{ totalCount }}</span>
  </button>
</template>

<style scoped>
.sac {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 9px;
  background: transparent;
  border: 1px solid color-mix(in srgb, var(--color-primary) 34%, transparent);
  border-radius: 999px;
  color: inherit;
  font-size: 12px;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s;
}
.sac:hover:not(:disabled) {
  background: color-mix(in srgb, var(--color-primary) 10%, transparent);
  border-color: var(--color-primary);
}
.sac:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.sac.checked,
.sac.partial {
  border-color: var(--color-primary);
  color: var(--color-primary-text);
}
/* 空圈圈 —— 需求明确要求未选中时是空心圆 */
.sac-box {
  width: 15px;
  height: 15px;
  border-radius: 50%;
  border: 1.5px solid currentColor;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  transition: background 0.12s;
}
.sac.checked .sac-box {
  background: var(--color-primary);
  border-color: var(--color-primary);
}
.sac.partial .sac-box {
  background: color-mix(in srgb, var(--color-primary) 22%, transparent);
}
.sac-check {
  color: var(--color-on-primary);
  font-size: 11px;
  line-height: 1;
  font-weight: 700;
}
.sac-dash {
  color: var(--color-primary-text);
  font-size: 13px;
  line-height: 1;
  font-weight: 700;
}
.sac-label {
  white-space: nowrap;
}
.sac-count {
  font-size: 11px;
  opacity: 0.7;
  font-variant-numeric: tabular-nums;
}
</style>
