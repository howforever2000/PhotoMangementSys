<script setup lang="ts">
/**
 * UiWidget —— 仪表墙基类组件（FEAT-110 建 / FEAT-116 加声部）
 *
 * 职责边界（保持原子性）：
 *   只管五件事 —— 材质（玻璃/普通/**裸**）、栅格占位（span/rows）、圆角壁厚、
 *   标题槽、媒体排布（下出血 / 满幅）。具体展示什么一律交给默认插槽（各 *Core），
 *   **禁止在本组件里写业务内容**。
 *
 * FEAT-116 为什么多出 "bare" 这一档
 * ─────────────────────────────────────────────────────────────────────
 * 上一版所有块都是玻璃板 —— 一屏十一块同重量的矩形，层级只能靠字号硬撑。
 * 参考图（Rainmeter 皮肤）里，"几点 / 几号"这类纯读数**没有容器**，
 * 只有照片、日历这类"有内容"的块才有板。于是这里把材质拆成两个声部：
 *   panel（默认）：有内容 → 玻璃板
 *   bare         ：纯读数 → 文字直接压在页面上
 * 板与裸交替出现时，"板"才重新被读成"承载内容的物件"，而不是默认背景纹理。
 */
import { computed } from "vue";
import { useThemeStore } from "../../../stores/theme";

const props = withDefaults(
  defineProps<{
    /** 横向占位列数（12 列栅格） */
    span?: 3 | 4 | 5 | 6 | 7 | 8 | 12;
    /** 纵向占位行高（84px/行）；窄屏下统一交给内容高度 */
    rows?: 1 | 2 | 3;
    /** 材质单次覆盖；不传则跟随主题设置（仅 panel 生效） */
    material?: "glass" | "plain";
    /** 声部：panel = 有板；bare = 无板（纯读数） */
    variant?: "panel" | "bare";
    /** 媒体下出血：内容铺到板的下/左/右三边（照片类面板用） */
    bleed?: boolean;
    /** 满幅：整块板就是媒体，标题浮在其上（那年今日等） */
    cover?: boolean;
    /** 英文小标签（9.5px 大写字距）；与 labelCn 二选一 */
    label?: string;
    /** 中文小标题 */
    labelCn?: string;
    /** 整块点击跳转的站内路径 */
    to?: string;
    /** 整块可点（监听 @click；与 to 二选一） */
    clickable?: boolean;
  }>(),
  {
    span: 4,
    rows: 2,
    material: undefined,
    variant: "panel",
    bleed: false,
    cover: false,
    label: "",
    labelCn: "",
    to: "",
    clickable: false,
  },
);

const theme = useThemeStore();

const isBare = computed(() => props.variant === "bare");

const matClass = computed(() => {
  if (isBare.value) return "";
  return (props.material ?? theme.widgetMaterial) === "plain"
    ? "ui-widget--plain"
    : "ui-widget--glass";
});

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
    :class="[
      `span-${span}`,
      `rows-${rows}`,
      matClass,
      {
        'ui-widget--bare': isBare,
        'ui-widget--bleed': bleed,
        'ui-widget--cover': cover,
        'ui-widget--link': to || clickable,
      },
    ]"
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
