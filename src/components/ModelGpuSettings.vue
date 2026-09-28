<script setup lang="ts">
/**
 * 识别性能设置（⚙ 页签的内容）—— 分区式布局
 *
 * 这一屏**只放与「这台机器怎么跑得更快」有关的开关**（适配不同电脑）：
 *   ① 本机硬件   —— GPU 加速、CPU 线程数（依赖识别服务）
 *   ② 语义模型   —— 档位切换（依赖识别服务，换档要重建索引）
 *   ③ 高级选项   —— 扫描批次 + 实测诊断（默认折叠，不打扰首次使用者）
 * 与网络有关的模型下载 / 镜像源**不在这里**，由「📦 模型管理」页签单独提供
 * （见 settings/PerfSettingsDialog.vue）——服务挂了也能下载。
 *
 * 结构说明（组件拆分，单文件超 300 行即拆）：
 *   原来 1054 行的单体被拆成 4 个自包含分区，各自负责自己的加载/错误/重试，
 *   共享数据直接读 Pinia（gpuStatus / vcrThreads / vcrModels / modelDownloads），
 *   因此分区之间不需要 prop 传递，也不会出现「改了一处另一处还是旧值」。
 *
 * 服务可用性：由 ClipTierSection 充当探针（`list_vcr_models` 最轻且必然依赖服务）。
 * 它失败 → 本组件显示横幅并锁定①③；模型管理页签不受影响（BUG-2026-0918-002）。
 */
import { ref } from "vue";
import CollapseSection from "./CollapseSection.vue";
import HardwareSection from "./settings/HardwareSection.vue";
import ClipTierSection from "./settings/ClipTierSection.vue";
import ScanAdvancedSection from "./settings/ScanAdvancedSection.vue";
import BenchmarkSection from "./settings/BenchmarkSection.vue";

/** 识别服务不可用 → 横幅 + 锁定依赖它的分区（不整块隐藏，用户仍能重试） */
const initFailed = ref(false);
const initError = ref("");
/** 重试令牌：自增触发 ClipTierSection 重新加载（不把函数当 prop 传） */
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
</script>

<template>
  <div class="mgps-wrap">
    <!-- 服务不可用：锁定而非隐藏（BUG-2026-0918-002） -->
    <p v-if="initFailed" class="banner">
      <b>识别服务暂不可用。</b>
      <span v-if="initError" class="banner-err">{{ initError }}</span>
      <span v-else>可能正在启动 / 加载模型，稍候重试。</span>
      <button class="btn btn-sm" @click="retry">🔄 重试</button>
      <br />
      <span class="banner-sub">
        下面①②③区依赖识别服务，已暂时锁定；<b>「📦 模型管理」页签不依赖服务，仍可正常使用</b>。
      </span>
    </p>

    <!-- ① 本机硬件 -->
    <section class="block">
      <HardwareSection :locked="initFailed" />
    </section>

    <section class="block">
      <ClipTierSection
        :locked="initFailed"
        :retry-token="retryToken"
        @failed="onServiceFail"
        @recovered="onServiceOk"
      />
    </section>

    <!-- ③ 高级选项：默认折叠，状态记在 localStorage（下次进入保持） -->
    <CollapseSection
      title="高级选项"
      subtitle="扫描批次 · 实测与诊断"
      storage-key="perf-advanced"
    >
      <section class="block">
        <ScanAdvancedSection />
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
  border-top: 1px solid var(--color-border);
  padding-top: 14px;
}
.block-gap {
  margin-top: 6px;
}
.banner {
  margin: 0;
  padding: 10px 12px;
  border: 1px solid #f2c94c;
  border-radius: var(--radius-sm, 8px);
  background: var(--color-warn-soft);
  color: var(--color-text);
  font-size: 12.5px;
  line-height: 1.7;
}
.banner-err {
  color: var(--color-danger);
}
.banner-sub {
  color: var(--color-text-2);
}
.btn {
  min-height: 26px;
  margin-left: 6px;
  padding: 3px 10px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm, 8px);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}
.btn:hover {
  border-color: var(--color-primary);
  color: var(--color-primary);
}
.btn:focus-visible {
  outline: 2px solid rgba(57, 108, 216, 0.55);
  outline-offset: 2px;
}
</style>
