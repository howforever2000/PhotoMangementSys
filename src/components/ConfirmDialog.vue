<script setup lang="ts">
/**
 * 通用二次确认对话框 —— 替换原生 confirm()
 *
 * 用于删除等危险操作：显示标题 + 明确后果提示 + 红色危险按钮，
 * 避免误点；点击遮罩、Esc 键或取消按钮关闭。
 * A11y：显示后自动聚焦到取消按钮（避免误触危险按钮）。
 */
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useThemeStore } from "../stores/theme";

const props = defineProps<{
  visible: boolean;
  /** 标题（如「删除相册」） */
  title: string;
  /** 正文（说明操作对象与后果） */
  message: string;
  /** 确认按钮文字（默认「删除」） */
  confirmText?: string;
  /** 取消按钮文字（默认「取消」） */
  cancelText?: string;
  /** 是否危险操作（确认按钮红色，默认 true） */
  danger?: boolean;
}>();

const emit = defineEmits<{
  (e: "confirm"): void;
  (e: "cancel"): void;
}>();

const theme = useThemeStore();
const cancelBtnEl = ref<HTMLButtonElement | null>(null);

/** ESC 键关闭（document 级监听，div 不获焦无法捕获） */
function onKey(e: KeyboardEvent) {
  if (e.key === "Escape" && props.visible) {
    // 同时阻止默认与传播：全局 ESC 逻辑（router.back）不会随后触发
    e.preventDefault();
    e.stopPropagation();
    emit("cancel");
  }
}

onMounted(() => {
  document.addEventListener("keydown", onKey);
});
onBeforeUnmount(() => {
  document.removeEventListener("keydown", onKey);
});

/** 打开后下一帧聚焦取消按钮（避免误触确认/危险键） */
watch(
  () => props.visible,
  (v) => {
    if (!v) return;
    requestAnimationFrame(() => cancelBtnEl.value?.focus());
  },
);
</script>

<template>
  <Teleport to="body">
    <Transition name="confirm-fade">
      <div
        v-if="visible"
        class="confirm-mask"
        @click.self="emit('cancel')"
      >
        <div
          class="confirm-dialog"
          role="dialog"
          aria-modal="true"
          :style="{
            background: theme.isDark ? 'rgba(30,34,46,.96)' : '#fff',
            border: `1px solid ${theme.isDark ? 'rgba(255,255,255,.09)' : 'rgba(0,0,0,.07)'}`,
          }"
        >
          <div class="confirm-title" :style="{ color: theme.textColor }">⚠️ {{ title }}</div>
          <div class="confirm-msg" :style="{ color: theme.subTextColor }">{{ message }}</div>
          <div class="confirm-actions">
            <button
              ref="cancelBtnEl"
              class="btn btn-cancel"
              :style="{
                background: theme.isDark ? 'rgba(255,255,255,.06)' : '#fff',
                color: theme.isDark ? '#e6e9f5' : '#555',
              }"
              @click="emit('cancel')"
            >
              {{ cancelText || "取消" }}
            </button>
            <button
              class="btn"
              :class="danger !== false ? 'btn-danger' : 'btn-primary'"
              @click="emit('confirm')"
            >
              {{ confirmText || "删除" }}
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.confirm-mask {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.confirm-dialog {
  width: 380px;
  max-width: calc(100vw - 48px);
  background: #fff;
  border-radius: 14px;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.25);
  padding: 22px 24px;
}

.confirm-title {
  font-size: 16px;
  font-weight: 700;
  color: #2c3e50;
  margin-bottom: 10px;
}

.confirm-msg {
  font-size: 13px;
  line-height: 1.8;
  color: #666;
  white-space: pre-line;
  margin-bottom: 20px;
}

.confirm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
}

.btn {
  padding: 8px 18px;
  font-size: 13px;
  border-radius: 8px;
  border: 1px solid #ddd;
  cursor: pointer;
  transition: all 0.15s;
}

.btn-cancel {
  background: #fff;
  color: #555;
}

.btn-cancel:hover {
  background: #f5f5f5;
}

.btn-danger {
  background: #e5484d;
  color: #fff;
  border-color: #e5484d;
}

.btn-danger:hover {
  background: #d13438;
}

.btn-primary {
  background: #396cd8;
  color: #fff;
  border-color: #396cd8;
}

/* 过渡动画 */
.confirm-fade-enter-active,
.confirm-fade-leave-active {
  transition: opacity 0.2s ease;
}

.confirm-fade-enter-active .confirm-dialog,
.confirm-fade-leave-active .confirm-dialog {
  transition: transform 0.2s ease;
}

.confirm-fade-enter-from,
.confirm-fade-leave-to {
  opacity: 0;
}

.confirm-fade-enter-from .confirm-dialog,
.confirm-fade-leave-to .confirm-dialog {
  transform: scale(0.95);
}
</style>
