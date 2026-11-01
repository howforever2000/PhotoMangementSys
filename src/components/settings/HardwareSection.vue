<script setup lang="ts">
/**
 * 本机硬件 —— 只读一行事实，不放任何按钮
 *
 * 以前这里挂着两个控件（GPU 加速开关 + CPU 线程数），而「GPU 加速」与本页的
 * 「加速」分区重复表达同一件事，用户分不清谁管谁（FEAT-139 收掉）。
 * 现在：硬件只报事实；开关统一在「加速」；线程数降级到「高级」（模型走 GPU 后
 * 它基本不影响速度，CPU 主要在做缩略图 / EXIF / 地点解码）。
 */
import { computed } from "vue";
import { useContentStore } from "../../stores/content";

const store = useContentStore();

const cores = computed(() => {
  const t = store.vcrThreads;
  if (!t) return "";
  const hy = t.hybrid && t.p_cores ? `${t.p_cores}P+${t.e_cores}E` : `${t.physical ?? t.logical} 核`;
  return hy;
});

const npu = computed(() => {
  const a = store.accel;
  if (!a) return "";
  if (a.available?.NPU) return "NPU 可用";
  if (a.ov?.installed) return "NPU 不可用";
  return "NPU 未启用";
});

const gpu = computed(() => {
  const a = store.accel;
  if (!a) return "";
  return a.available?.GPU ? "GPU 可用" : "无 GPU 加速";
});
</script>

<template>
  <div class="hw">
    <h3 class="hw-title">本机硬件</h3>
    <span v-if="cores" class="hw-item">CPU {{ cores }}</span>
    <span v-if="npu" class="hw-item" :class="{ ok: store.accel?.available?.NPU }">{{ npu }}</span>
    <span v-if="gpu" class="hw-item" :class="{ ok: store.accel?.available?.GPU }">{{ gpu }}</span>
  </div>
</template>

<style scoped>
.hw {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.hw-title {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  color: var(--color-text);
}
.hw-item {
  font-size: 12.5px;
  color: var(--color-text-3);
}
.hw-item.ok {
  color: var(--color-text-2);
}
</style>
