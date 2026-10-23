<script setup lang="ts">
/**
 * 通用空状态（FEAT-100 / impeccable polish）
 *
 * 之前相册列表、时间线、回忆页各写一套：emoji 52–56px、标题 14–18px、
 * 动作按钮有的套 .btn、有的裸 button，视觉与命中区都不一致。
 * 这里收口成一套共享组件：图标 52px、标题 18/600、说明 13.5/1.65、主按钮 38px。
 *
 * 动作按钮刻意用 --color-primary + 深墨字（与登录页金底墨字同一对比度口径），
 * 不依赖各页缺失的 .btn 基础样式。
 */
withDefaults(
  defineProps<{
    /** 装饰性 emoji（不参与读屏；标题负责表达语义） */
    icon?: string;
    title: string;
    description?: string;
    /** 主按钮文案；为空则不渲染按钮 */
    actionLabel?: string;
    /** error 只影响图标/标题的紧急感，不改变版式 */
    tone?: "default" | "error";
  }>(),
  {
    icon: "📭",
    description: "",
    actionLabel: "",
    tone: "default",
  },
);

const emit = defineEmits<{ action: [] }>();
</script>

<template>
  <div class="empty-state" :class="`empty-${tone}`">
    <div v-if="icon" class="empty-icon" aria-hidden="true">{{ icon }}</div>
    <h2 class="empty-title">{{ title }}</h2>
    <p v-if="description" class="empty-desc">{{ description }}</p>
    <button
      v-if="actionLabel"
      class="empty-action"
      type="button"
      @click="emit('action')"
    >
      {{ actionLabel }}
    </button>
    <slot name="action" />
  </div>
</template>

<style scoped>
.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  padding: 72px 20px;
  text-align: center;
  color: var(--color-on-bg, var(--color-text));
}

.empty-icon {
  font-size: 52px;
  line-height: 1;
  filter: saturate(0.85) drop-shadow(0 6px 14px rgba(0, 0, 0, 0.2));
}

.empty-title {
  margin: 0;
  font-size: 18px;
  font-weight: 600;
  letter-spacing: 0;
  color: var(--color-on-bg, var(--color-text));
}

.empty-desc {
  max-width: 460px;
  margin: 0;
  font-size: 13.5px;
  line-height: 1.65;
  color: var(--color-on-bg-2, var(--color-text-2));
}

.empty-action {
  min-height: 38px;
  margin-top: 6px;
  padding: 0 18px;
  border: none;
  border-radius: 10px;
  background: var(--color-primary);
  /* 金底墨字：白字在 #e8a33d 上仅 2.2:1，深墨字约 6.6:1 */
  color: #1f2733;
  font-size: 13.5px;
  font-weight: 600;
  cursor: pointer;
  transition: background var(--dur-2), transform var(--dur-1);
}

.empty-action:hover {
  background: var(--color-primary-hover);
}

.empty-action:active {
  transform: translateY(1px);
}

.empty-action:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--color-primary) 70%, transparent);
  outline-offset: 3px;
}

.empty-error .empty-title {
  color: var(--color-on-bg, var(--color-danger, var(--color-text)));
}

@media (max-width: 640px) {
  .empty-state {
    padding: 48px 16px;
  }
  .empty-icon {
    font-size: 46px;
  }
  .empty-title {
    font-size: 17px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .empty-action {
    transition: none;
  }
}
</style>
