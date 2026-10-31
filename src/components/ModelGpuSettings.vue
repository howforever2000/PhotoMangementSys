<script setup lang="ts">
/**
 * 识别性能设置（⚙ 页签）—— 只回答一个问题：**这台机器把哪些计算放在哪个引擎上**
 *
 * 布局（自上而下 = 从「是什么」到「怎么调」）：
 *   本机硬件   一行事实（核数 / NPU / 核显），无按钮
 *   加速       两个独立勾选（NPU / 核显）+ 「哪条计算跑在哪个引擎」对照表
 *   人脸模型   档位选择（精度 ↔ 速度，换档要重建人物库）
 *   高级       扫描批次 / CPU 线程数 / 测速与诊断（默认折叠）
 *
 * FEAT-140：语义模型那一行整块去掉 —— 只剩一档（FEAT-137），没有「选」这件事；
 * 服务探针改由 AccelSection 承担（它必调 /accel，最轻且一定依赖服务）。
 *
 * FEAT-139 收掉的东西：
 *   · 旧的「GPU 加速」开关 —— 与「加速」分区表达的是同一件事，留两个开关只会让人
 *     猜「到底哪个生效」；现在分配表的默认值就包含「没有 NPU 时全部走核显」。
 *   · 顶部「这台机器现在怎么跑」摘要 —— 与对照表重复。
 *   · 顶部把 CPU 线程数摆在第一位 —— 模型默认走 GPU/NPU 后，这个旋钮基本不影响
 *     速度（CPU 主要在生成缩略图、扫 EXIF、解地点），降级到「高级」。
 *
 * 服务可用性：AccelSection 充当探针（`/accel` 最轻且必然依赖服务）。
 * 它失败 → 横幅 + 锁定依赖服务的分区；「📦 模型管理」页签不受影响。
 */
import { ref, onMounted } from "vue";
import CollapseSection from "./CollapseSection.vue";
import HardwareSection from "./settings/HardwareSection.vue";
import AccelSection from "./settings/AccelSection.vue";
import FaceTierSection from "./settings/FaceTierSection.vue";
import ThreadsSection from "./settings/ThreadsSection.vue";
import ScanAdvancedSection from "./settings/ScanAdvancedSection.vue";
import BenchmarkSection from "./settings/BenchmarkSection.vue";
import { useContentStore } from "../stores/content";

const store = useContentStore();

/** 识别服务不可用 → 横幅 + 锁定依赖它的分区（不整块隐藏，用户仍能重试） */
const initFailed = ref(false);
const initError = ref("");
/** 重试令牌：自增触发下游分区重新加载（不把函数当 prop 传） */
const retryToken = ref(0);

function onServiceFail(msg: string) {
  initFailed.value = true;
  initError.value = msg;
}
function onServiceOk() {
  initFailed.value = false;
  initError.value = "";
}
function retry() {
  initFailed.value = false;
  initError.value = "";
  retryToken.value += 1;
}

onMounted(() => {
  // 只做展示：失败不弹错（分区各自有错误态与重试）
  void store.fetchAccel().catch(() => {});
  void store.fetchVcrModels().catch(() => {});
  void store.fetchVcrThreads().catch(() => {});
});
</script>

<template>
  <div class="mgps-wrap">
    <!-- 服务不可用：锁定而非隐藏（BUG-2026-0918-002） -->
    <p v-if="initFailed" class="banner">
      <b>识别服务暂不可用</b>
      <span v-if="initError" class="banner-err">{{ initError }}</span>
      <button class="btn btn-sm" @click="retry">重试</button>
      <span class="banner-sub">「📦 模型管理」页签不受影响，可正常下载。</span>
    </p>

    <section class="block">
      <HardwareSection />
    </section>

    <section class="block">
      <AccelSection :locked="initFailed" @failed="onServiceFail" @recovered="onServiceOk" />
    </section>

    <section class="block">
      <FaceTierSection :locked="initFailed" :retry-token="retryToken" />
    </section>

    <CollapseSection title="高级" subtitle="扫描批次 · 线程数 · 测速诊断" storage-key="perf-advanced">
      <section class="block">
        <ScanAdvancedSection />
      </section>
      <section class="block block-gap">
        <ThreadsSection :locked="initFailed" />
      </section>
      <section class="block block-gap">
        <BenchmarkSection :locked="initFailed" />
      </section>
    </CollapseSection>
  </div>
</template>

<style scoped>
.mgps-wrap {
  display: flex;
  flex-direction: column;
  gap: 14px;
  font-size: 13px;
  color: var(--color-text);
}
.block {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding-bottom: 2px;
}
/* 分区间隔：用分隔线而不是再套一层卡片，避免「卡中卡」 */
.block + .block {
  border-top: 1px solid color-mix(in srgb, var(--color-border) 60%, transparent);
  padding-top: 14px;
}
.block-gap {
  margin-top: 12px;
}
.banner {
  margin: 0;
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  padding: 8px 10px;
  border-radius: var(--radius-sm, 8px);
  background: var(--color-warn-soft, rgba(255, 180, 0, 0.12));
  font-size: 12.5px;
}
.banner-err {
  color: var(--color-danger, #d9534f);
  overflow-wrap: anywhere;
}
.banner-sub {
  font-size: 12px;
  color: var(--color-text-3);
}
.btn {
  min-height: 26px;
  padding: 3px 10px;
  border-radius: var(--radius-sm, 8px);
  border: 1px solid var(--color-border);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}
.btn-sm {
  min-height: 24px;
  padding: 2px 8px;
}
</style>
