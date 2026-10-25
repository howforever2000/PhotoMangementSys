<script setup lang="ts">
/**
 * UiWidget —— 仪表墙基类组件（FEAT-110）
 *
 * 职责边界（保持原子性）：
 *   只管四件事 —— 材质（玻璃/普通）、栅格占位（span/rows）、圆角壁厚、标题槽。
 *   具体展示什么一律交给默认插槽（各 *Core 组件），**禁止在本组件里写业务内容**。
 *
 * 材质来源：theme.widgetMaterial（全局偏好）→ 可被 prop material 单次覆盖。
 * 交互：传 to（站内路径）整块可点；或监听 @click。纯展示就两者都不传。
 */
import { computed } from "vue";
import { useThemeStore } from "../../../stores/theme";

const props = withDefaults(
  defineProps<{
    /** 横向占位列数（12 列栅格） */
    span?: 3 | 4 | 6 | 8 | 12;
    /** 纵向占位行高（86px/行）；窄屏下统一交给内容高度 */
    rows?: 1 | 2;
    /** 单次覆盖全局材质；不传则跟随主题设置 */
    material?: "glass" | "plain";
    /** 英文小标签（9.5px 大写字距）；与 labelCn 二选一 */
    label?: string;
    /** 中文小标题 */
    labelCn?: string;
    /** 整块点击跳转的站内路径 */
    to?: string;
    /** 整块可点（监听 @click；与 to 二选一） */
    clickable?: boolean;
  }>(),
  { span: 4, rows: 2, material: undefined, label: "", labelCn: "", to: "", clickable: false },
);

const theme = useThemeStore();

const matClass = computed(() =>
  (props.material ?? theme.widgetMaterial) === "plain"
    ? "ui-widget--plain"
    : "ui-widget--glass",
);

const emit = defineEmits<{ (e: "click"): void }>();
function activate() {
  emit("click");
}
</script>

<template>
  <component
    :is="to ? 'router-link' : 'section'"
    :to="to || undefined"
    class="ui-widget"
    :class="[`span-${span}`, `rows-${rows}`, matClass, { 'ui-widget--link': to || clickable }]"
    @click="to ? undefined : activate()"
  >
    <header v-if="label || labelCn || $slots.head || $slots.right" class="ui-widget-head">
      <slot name="head">
        <span v-if="label" class="ui-widget-label">{{ label }}</span>
        <span v-else-if="labelCn" class="ui-widget-label-cn">{{ labelCn }}</span>
      </slot>
      <span v-if="$slots.right" class="ui-widget-head-right"><slot name="right" /></span>
    </header>
    <div class="ui-widget-body">
      <slot />
    </div>
  </component>
</template>
