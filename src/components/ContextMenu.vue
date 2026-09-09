<script setup lang="ts">
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
  // 点击外部关闭
  document.addEventListener("click", onDocClick);
  document.addEventListener("contextmenu", onDocRightClick);
});

onBeforeUnmount(() => {
  document.removeEventListener("click", onDocClick);
  document.removeEventListener("contextmenu", onDocRightClick);
});

function onDocClick() {
  emit("close");
}

function onDocRightClick(e: MouseEvent) {
  // 如果右键点击在菜单内，不关闭
  if (menuRef.value?.contains(e.target as Node)) return;
  emit("close");
}

function onItemClick(item: ContextMenuItem) {
  if (item.disabled) return;
  item.onClick();
  emit("close");
}
</script>

<template>
  <Teleport to="body">
    <div
      class="ctx-overlay"
      @click.stop
      @contextmenu.prevent.stop
    >
      <div
        ref="menuRef"
        class="ctx-menu"
        :style="{ left: `${x}px`, top: `${y}px` }"
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
  background: rgba(57, 108, 216, 0.1);
  color: #2f5cc2;
}

.ctx-item.disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.ctx-item.danger {
  color: #e5484d;
}

.ctx-item.danger:hover:not(.disabled) {
  background: rgba(229, 72, 77, 0.1);
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
