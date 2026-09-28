<script setup lang="ts">
/**
 * ① 本机硬件 —— 性能设置的第一段（只放「这台机器跑得更快」的开关）
 *
 * 本文件只负责分区标题与布局；两个子分区各自独立加载/报错/重试：
 *   GpuSection     运行硬件探测 + GPU 加速开关
 *   ThreadsSection CPU 线程数（ONNX intra_op）
 * 拆开是为了单一职责：一个组件只管一件事，且任一数据源失败不影响另一个。
 */
import GpuSection from "./GpuSection.vue";
import ThreadsSection from "./ThreadsSection.vue";

/** 分区级 props：`locked` = 识别服务不可用时锁定本区 */
defineProps<{ locked?: boolean }>();
</script>

<template>
  <section class="sec">
    <div class="sec-head">
      <h3 class="sec-title">本机硬件</h3>
      <span class="sec-sub">决定这台机器怎么跑得更快</span>
    </div>

    <GpuSection :locked="locked" />
    <ThreadsSection :locked="locked" />
  </section>
</template>

<style scoped>
.sec {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.sec-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
}
.sec-title {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  color: var(--color-text);
}
.sec-sub {
  font-size: 12px;
  color: var(--color-text-3);
}
</style>
