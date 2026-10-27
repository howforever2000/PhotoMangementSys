<script setup lang="ts">
/**
 * MemoryCore —— 大封面回忆芯（FEAT-110）
 * 原子职责：一张铺满的封面 + 底部暗色渐变上的标题/副标题。点击整块由父级处理。
 */
withDefaults(
  defineProps<{
    src?: string;
    title: string;
    subtitle?: string;
  }>(),
  { src: "", subtitle: "" },
);
const emit = defineEmits<{ (e: "activate"): void }>();
</script>

<template>
  <button type="button" class="w-memory" @click="emit('activate')">
    <img v-if="src" :src="src" loading="lazy" alt="" />
    <div v-else class="w-memory-ph">🌅</div>
    <span class="w-memory-fade"></span>
    <span class="w-memory-meta">
      <b>{{ title }}</b>
      <small v-if="subtitle">{{ subtitle }}</small>
    </span>
  </button>
</template>

<style scoped>
.w-memory {
  flex: 1;
  position: relative;
  border: none;
  border-radius: var(--w-radius-inner);
  overflow: hidden;
  margin-top: 10px;
  padding: 0;
  min-height: 0;
  cursor: pointer;
  box-shadow: var(--cover-inset);
  background: rgba(128, 128, 128, 0.18);
}
.w-memory img,
.w-memory-ph {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.w-memory-ph {
  display: grid;
  place-items: center;
  font-size: 34px;
}
.w-memory-fade {
  position: absolute;
  inset: 0;
  background: linear-gradient(to top, rgba(0, 0, 0, 0.62) 0%, rgba(0, 0, 0, 0.12) 42%, transparent 60%);
}
.w-memory-meta {
  position: absolute;
  left: 12px;
  right: 12px;
  bottom: 11px;
  text-align: left;
}
.w-memory-meta b {
  display: block;
  font-size: 14px;
  font-weight: 650;
  color: #fff;
  text-shadow: 0 1px 4px rgba(0, 0, 0, 0.6);
  letter-spacing: -0.01em;
}
.w-memory-meta small {
  font-size: 10.5px;
  color: rgba(255, 255, 255, 0.82);
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.6);
}
</style>
