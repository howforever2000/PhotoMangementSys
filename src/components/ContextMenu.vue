<script setup lang="ts">
/**
 * 通用右键上下文菜单 —— 替换原生 contextmenu
 *
 * 关闭语义对齐 Windows 资源管理器（BUG-2026-1007-001 的修复要点）：
 * | 操作 | 行为 |
 * |------|------|
 * | 左键点菜单外任意处 | 关闭 |
 * | **右键点菜单外任意处** | **关闭旧菜单，并在新位置弹出新菜单**（Windows 原生行为） |
 * | 点菜单项 | 执行动作后关闭（动作抛异常也必须关闭） |
 * | 点菜单自身空白处 | 关闭 |
 * | <kbd>Esc</kbd> | 关闭 |
 *
 * 修复前的两个具体缺陷：
 * 1. 全屏遮罩写 `@click.stop` → 点外面永远到不了关闭分支，菜单「关不掉」；
 * 2. 遮罩写 `@contextmenu.prevent.stop` → 右键别处既不关闭、又被 preventDefault
 *    吃掉浏览器原生菜单，导致「右键打开新菜单」这个动作彻底做不到（菜单锁死）。
 */
import { onBeforeUnmount, onMounted, ref } from "vue";

export interface ContextMenuItem {
  label: string;
  icon?: string;
  /** danger = 红字警告 */
  danger?: boolean;
  disabled?: boolean;
  divider?: false;
  onClick: () => void;
}

export interface ContextMenuDivider {
  divider: true;
}

export type ContextMenuEntry = ContextMenuItem | ContextMenuDivider;

const props = defineProps<{
  items: ContextMenuEntry[];
  x: number;
  y: number;
}>();

const emit = defineEmits<{
  (e: "close"): void;
}>();

const menuRef = ref<HTMLElement | null>(null);
const overlayRef = ref<HTMLElement | null>(null);

onMounted(() => {
  // 边界修正：菜单超出视口时调整位置
  const menu = menuRef.value;
  if (menu) {
    const rect = menu.getBoundingClientRect();
    if (rect.right > window.innerWidth) {
      menu.style.left = `${props.x - rect.width}px`;
    }
    if (rect.bottom > window.innerHeight) {
      menu.style.top = `${props.y - rect.height}px`;
    }
  }
  // Esc 关闭：document 级监听（div 不获焦无法捕获）
  document.addEventListener("keydown", onKey);
});

onBeforeUnmount(() => {
  document.removeEventListener("keydown", onKey);
});

function onKey(e: KeyboardEvent) {
  if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    emit("close");
  }
}

/** 左键/右键点到菜单外：统一走关闭（菜单内子元素已 @click.stop，不会冒泡到这里） */
function onOverlayPointerDown(e: PointerEvent) {
  // 只处理主键与右键；中键等保持浏览器默认
  if (e.button !== 0 && e.button !== 2) return;
  emit("close");
}

/**
 * 右键点到菜单外：关闭旧菜单 + 在新位置重开（Windows 语义）
 *
 * 遮罩是 fixed 全屏，会挡住 `elementFromPoint`，所以先临时关掉它的
 * `pointer-events` 拿到真实目标元素，再合成一个 contextmenu 事件投递过去 ——
 * 由目标元素自己的 handler 决定弹出什么菜单（画廊的格子 / 相册卡片 / 缩略图…）。
 */
function onOverlayContextMenu(e: MouseEvent) {
  e.preventDefault();
  const x = e.clientX;
  const y = e.clientY;
  emit("close");
  // 等 Vue 把菜单卸载后再取，否则 elementFromPoint 仍命中自己
  requestAnimationFrame(() => {
    const overlay = overlayRef.value;
    const prev = overlay?.style.pointerEvents ?? "";
    if (overlay) overlay.style.pointerEvents = "none";
    const target = document.elementFromPoint(x, y);
    if (overlay) overlay.style.pointerEvents = prev;
    if (!target) return;
    target.dispatchEvent(
      new MouseEvent("contextmenu", {
        clientX: x,
        clientY: y,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
}

/**
 * 菜单项点击：先执行动作，再关闭
 *
 * 用 try/finally —— 修复前若 `onClick` 抛异常，`emit("close")` 会被跳过，
 * 菜单永久卡在屏幕上且无法用任何方式关掉（点哪都没反应）。
 */
function onItemClick(item: ContextMenuItem) {
  if (item.disabled) return;
  try {
    item.onClick();
  } finally {
    emit("close");
  }
}
</script>

<template>
  <Teleport to="body">
    <!--
      遮罩：透明全屏，只负责「点外面关闭 / 右键别处重开」的命中判定。
      注意不能再加 .stop —— 那会让 document 层的关闭分支永远收不到事件。
    -->
    <div
      ref="overlayRef"
      class="ctx-overlay"
      @pointerdown="onOverlayPointerDown"
      @contextmenu="onOverlayContextMenu"
    >
      <div
        ref="menuRef"
        class="ctx-menu"
        :style="{ left: `${x}px`, top: `${y}px` }"
        @pointerdown.stop
        @click.stop
      >
        <template v-for="(item, i) in items" :key="i">
          <div v-if="item.divider" class="ctx-divider" />
          <button
            v-else
            class="ctx-item"
            :class="{ disabled: item.disabled, danger: item.danger }"
            :disabled="item.disabled"
            @click="onItemClick(item)"
          >
            <span v-if="item.icon" class="ctx-icon">{{ item.icon }}</span>
            <span class="ctx-label">{{ item.label }}</span>
          </button>
        </template>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.ctx-overlay {
  position: fixed;
  inset: 0;
  z-index: 9999;
}

.ctx-menu {
  position: fixed;
  z-index: 10000;
  background: rgba(255, 255, 255, 0.97);
  border: 1px solid rgba(0, 0, 0, 0.1);
  border-radius: 10px;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.18);
  padding: 6px 0;
  min-width: 180px;
  max-width: 280px;
  backdrop-filter: blur(8px);
}

.ctx-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 14px;
  background: transparent;
  border: none;
  text-align: left;
  font-size: 13.5px;
  color: #2c3e50;
  cursor: pointer;
  transition: background 0.1s;
}

.ctx-item:hover:not(.disabled) {
  background: color-mix(in srgb, var(--color-primary) 10%, transparent);
  color: var(--color-primary-hover);
}

.ctx-item.disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.ctx-item.danger {
  color: var(--color-danger);
}

.ctx-item.danger:hover:not(.disabled) {
  background: color-mix(in srgb, var(--color-danger) 10%, transparent);
  color: #c0393b;
}

.ctx-icon {
  font-size: 14px;
  width: 18px;
  text-align: center;
  flex-shrink: 0;
}

.ctx-label {
  flex: 1;
}

.ctx-divider {
  height: 1px;
  background: rgba(0, 0, 0, 0.08);
  margin: 5px 8px;
}
</style>
