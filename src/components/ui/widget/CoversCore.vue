<script setup lang="ts">
/**
 * CoversCore —— 封面拼贴芯（FEAT-110）
 * 原子职责：把若干封面图摆成 mosaic（左大右两小，3 张）或 strip（均摊横条）。
 * 纯展示 + 可选点击；src 必须是已 convertFileSrc 的可渲染 URL。
 */
withDefaults(
  defineProps<{
    items: { src?: string; name?: string }[];
    variant?: "mosaic" | "strip";
  }>(),
  { variant: "mosaic" },
);
const emit = defineEmits<{ (e: "pick", index: number): void }>();
</script>

<template>
  <div class="w-covers" :class="variant">
    <div
      v-for="(it, idx) in items"
      :key="idx"
      class="w-cv"
      @click="emit('pick', idx)"
    >
      <img v-if="it.src" :src="it.src" loading="lazy" alt="" />
      <div v-else class="w-cv-ph">🖼</div>
      <span v-if="it.name" class="w-cv-name">{{ it.name }}</span>
    </div>
  </div>
</template>

<style scoped>
.w-covers {
  flex: 1;
  display: grid;
  gap: 6px;
  min-height: 0;
  margin-top: 10px;
}
.w-covers.mosaic {
  grid-template-columns: 1.6fr 1fr;
  grid-template-rows: 1fr 1fr;
}
.w-covers.mosaic .w-cv:first-child {
  grid-row: span 2;
}
.w-covers.strip {
  display: flex;
}
.w-covers.strip .w-cv {
  flex: 1;
  min-width: 0;
}
.w-cv {
  cursor: pointer;
}
</style>
