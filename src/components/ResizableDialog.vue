<script setup lang="ts">
/**
 * FEAT-096（R3）：可拖边缩放的窗口容器
 *
 * 需求：「所有照片的预览框（点击人物头像或其他地方出现的子图）应当支持拖动边界放大缩小，
 * 参考 windows 软件框」+「原图预览界面不能全屏 / 不能拖动边界放大缩小」。
 *
 * 为什么抽成组件而不是各弹窗各写一遍：
 * 8 个把手 + 标题栏拖动 + 双击最大化 + 视口收拢，逻辑约 200 行；
 * 人物照片弹窗 / 分类网格 / 地点网格 / 原图预览器四处都要，
 * 复制四份必然出现「有的能拖有的不能拖」的不一致。
 *
 * 用法（见 PersonGallery / CategoryGallery / LocationGallery / PhotoLightbox）：
 * ```
 * <ResizableDialog v-slot="{ head, body }" :initial-width="720" ...>
 *   ...
 * </ResizableDialog>
 * ```
 * 插槽方案而非一堆布尔 props —— 保持「组合优于配置」。
 */
import { computed, onMounted } from "vue";
import { useResizableDialog } from "../composables/useResizableDialog";

const props = withDefaults(
  defineProps<{
    /** 初始宽度（px） */
    initialWidth?: number;
    /** 初始高度（px） */
    initialHeight?: number;
    /** 最小宽度（px） */
    minWidth?: number;
    /** 最小高度（px） */
    minHeight?: number;
    /** 是否允许拖动 / 缩放 */
    resizable?: boolean;
    /** 内容区额外的 class（供各弹窗挂自己的样式） */
    bodyClass?: string;
  }>(),
  {
    initialWidth: 760,
    initialHeight: 560,
    minWidth: 420,
    minHeight: 260,
    resizable: true,
    bodyClass: "",
  },
);

const rs = useResizableDialog({
  initialWidth: props.initialWidth,
  initialHeight: props.initialHeight,
  minWidth: props.minWidth,
  minHeight: props.minHeight,
  // 用 computed 转成 Ref，composable 内部 watch 它来决定是否居中
  enabled: computed(() => props.resizable),
});

// 首帧就居中（弹窗都是 v-if 挂载，挂载即居中符合直觉）
onMounted(() => rs.ensureCentered());

/** 把手方向 → CSS class（模板里 v-for 渲染 8 个） */
const HANDLES = ["n", "s", "e", "w", "ne", "nw", "se", "sw"] as const;
</script>

<template>
  <div
    class="rzd"
    :class="{ 'rzd-dragging': rs.dragging.value, 'rzd-fullscreen': rs.fullscreen.value, 'rzd-static': !resizable }"
    :style="rs.sizeStyle.value"
  >
    <!-- 8 向把手：仅在可缩放且非全屏时渲染（全屏时无可拖的边） -->
    <template v-if="resizable && !rs.fullscreen.value">
      <span
        v-for="h in HANDLES"
        :key="h"
        class="rzd-grip"
        :class="`rzd-grip-${h}`"
        @pointerdown="rs.handleDown($event, h)"
        @pointermove="rs.onPointerMove"
        @pointerup="rs.onPointerUp"
        @pointercancel="rs.onPointerUp"
      />
    </template>

    <!-- 标题栏：按住拖动移动窗口；双击 = 全屏/还原 -->
    <div
      class="rzd-head"
      @pointerdown="rs.titleDown"
      @pointermove="rs.onPointerMove"
      @pointerup="rs.onPointerUp"
      @pointercancel="rs.onPointerUp"
      @dblclick="rs.onTitleDoubleClick"
    >
      <slot name="head" :fullscreen="rs.fullscreen.value" :toggle-fullscreen="rs.toggleFullscreen" />
    </div>

    <!-- 内容区 -->
    <div class="rzd-body" :class="bodyClass">
      <slot />
    </div>
  </div>
</template>

<style scoped>
/*
 * 容器用 position: absolute —— 外层遮罩是 fixed 全屏 flex 居中，
 * absolute 相对遮罩定位，拖到哪就是哪。
 */
.rzd {
  position: absolute;
  display: flex;
  flex-direction: column;
  background: transparent; /* 底色由 surfaceStyle(主题) 或 bodyClass 提供 */
  border-radius: 14px;
  box-shadow: 0 18px 52px rgba(0, 0, 0, 0.32);
  overflow: hidden;
  /* 拖动时禁用文本选中，避免鼠标划过出现蓝块 */
  user-select: none;
}
.rzd.rzd-dragging {
  /* 拖动中关掉 transition，否则每一帧都在补间导致窗口"粘"住 */
  transition: none;
  cursor: grabbing;
}
.rzd.rzd-fullscreen {
  border-radius: 0;
}
.rzd.rzd-static {
  cursor: default;
}

.rzd-head {
  flex-shrink: 0;
  cursor: grab;
  position: relative;
  z-index: 2;
}
.rzd.rzd-dragging .rzd-head {
  cursor: grabbing;
}

.rzd-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  position: relative;
  z-index: 1;
}

/* ---------------- 8 向把手 ---------------- */
.rzd-grip {
  position: absolute;
  z-index: 5;
}
.rzd-grip:hover {
  /* 悬停时给一点淡高亮，让用户知道这条边可拖 */
  background: color-mix(in srgb, var(--color-primary) 45%, transparent);
}
.rzd-grip-n,
.rzd-grip-s {
  left: 10px;
  right: 10px;
  height: 6px;
  cursor: ns-resize;
}
.rzd-grip-n { top: 0; }
.rzd-grip-s { bottom: 0; }
.rzd-grip-e,
.rzd-grip-w {
  top: 10px;
  bottom: 10px;
  width: 6px;
  cursor: ew-resize;
}
.rzd-grip-e { right: 0; }
.rzd-grip-w { left: 0; }
.rzd-grip-ne,
.rzd-grip-nw,
.rzd-grip-se,
.rzd-grip-sw {
  width: 14px;
  height: 14px;
}
.rzd-grip-ne { top: 0; right: 0; cursor: nesw-resize; }
.rzd-grip-sw { bottom: 0; left: 0; cursor: nesw-resize; }
.rzd-grip-nw { top: 0; left: 0; cursor: nwse-resize; }
.rzd-grip-se { bottom: 0; right: 0; cursor: nwse-resize; }
</style>
