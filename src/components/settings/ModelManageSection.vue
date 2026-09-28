<script setup lang="ts">
/**
 * 📦 模型管理（页签内容）
 *
 * 与「⚙ 性能设置」彻底分屏：这里是**网络**问题（下载 / 镜像源），不是硬件问题；
 * 且它**不依赖识别服务** —— 服务没起来照样能下（①②③区在那种情况下是锁定的）。
 *
 * 子分区各管一摊：ModelDownloadsSection（下载任务 + 进度）、
 * ModelSourcesSection（源连通性自检 + 自定义源模板）。
 */
import ModelDownloadsSection from "./ModelDownloadsSection.vue";
import ModelSourcesSection from "./ModelSourcesSection.vue";
</script>

<template>
  <section class="sec">
    <p class="lead">
      模型缺失会导致对应识别通道降级（例如没有 SCRFD 就不给人脸标号）。这里只负责把模型取到本机，
      <b>与「这台机器怎么跑得更快」无关</b>的设置在隔壁页签。
    </p>

    <ModelDownloadsSection />

    <div class="sep"></div>

    <ModelSourcesSection />
  </section>
</template>

<style scoped>
.sec {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.lead {
  margin: 0;
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--color-text-3);
}
.sep {
  border-top: 1px solid var(--color-border);
}
</style>
