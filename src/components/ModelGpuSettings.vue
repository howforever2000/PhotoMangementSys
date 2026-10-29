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
import { ref, computed, onMounted } from "vue";
import CollapseSection from "./CollapseSection.vue";
import HardwareSection from "./settings/HardwareSection.vue";
import OvRuntimeSection from "./settings/OvRuntimeSection.vue";
import ClipTierSection from "./settings/ClipTierSection.vue";
import FaceTierSection from "./settings/FaceTierSection.vue";
import ScanAdvancedSection from "./settings/ScanAdvancedSection.vue";
import BenchmarkSection from "./settings/BenchmarkSection.vue";
import { useContentStore } from "../stores/content";

const store = useContentStore();

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

// ---- FEAT-133：「这台机器现在怎么跑」摘要 ----
//
// 这一屏原来难懂的核心：四个开关分属**两条不同的腿**（GPU 加速 → 人物/人脸/OCR；
// 语义索引 → 另一个后端），但 UI 上从没说清楚。现在在顶部用三行把“当前实际在哪跑”
// 直接摆出来，开关变成“为什么这样”的解释而不是猜谜。
//
// 数据源：gpuStatus（人物链路的请求值）+ clipRuntime（语义链路的服务端事实）。
// 两者都不触发模型加载，打开设置弹窗不会变慢。
const accelOn = computed(() => store.gpuStatus?.use_gpu === true);
const semantic = computed(() => {
  const c = store.clipRuntime;
  if (!c) return { text: "读取中…", tone: "dim" as const };
  if (c.active) {
    const dev = c.device === "NPU" ? "NPU" : c.device === "GPU" ? "核显" : c.device || "CPU";
    return { text: `OpenVINO · ${dev}`, tone: "ok" as const };
  }
  if (c.mode === "ort" && c.runtime?.installed) {
    return { text: "ONNX Runtime（已关加速）", tone: "dim" as const };
  }
  return { text: "ONNX Runtime · CPU", tone: "dim" as const };
});
const personChain = computed(() =>
  accelOn.value
    ? { text: "DirectML 加速", tone: "ok" as const }
    : { text: "CPU（未开加速）", tone: "dim" as const },
);

onMounted(() => {
  // 摘要只做展示：失败不弹错（下面的分区各自有自己的错误态与重试）
  if (!store.clipRuntime) void store.fetchClipRuntime().catch(() => {});
});
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

    <!-- FEAT-133：这台机器现在怎么跑（把两条腿分开说清） -->
    <div class="now" aria-label="这台机器当前的推理方式">
      <div class="now-row">
        <span class="now-k">语义索引</span>
        <span class="now-v" :class="semantic.tone">{{ semantic.text }}</span>
        <span class="now-note">照片编向量 · 以图搜图 · 语义分类</span>
      </div>
      <div class="now-row">
        <span class="now-k">人物与文字</span>
        <span class="now-v" :class="personChain.tone">{{ personChain.text }}</span>
        <span class="now-note">人物检测 · 人脸 · OCR（由下方「GPU 加速」控制）</span>
      </div>
    </div>

    <!-- ① 本机硬件 -->
    <section class="block">
      <HardwareSection :locked="initFailed" />
    </section>

    <!-- ①b 语义索引加速（OpenVINO 可选组件；只作用于语义图像塔） -->
    <section class="block">
      <OvRuntimeSection :locked="initFailed" />
    </section>

    <section class="block">
      <ClipTierSection
        :locked="initFailed"
        :retry-token="retryToken"
        @failed="onServiceFail"
        @recovered="onServiceOk"
      />
    </section>

    <!-- ②b 人脸模型档位（两档可切、选择持久化；换档后需重建人物库） -->
    <section class="block">
      <FaceTierSection :locked="initFailed" :retry-token="retryToken" />
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
/* 「现在怎么跑」摘要：两条腿分两行，每行只回答“现在在哪” */
.now {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 10px 12px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm, 8px);
  background: var(--color-surface-2);
}
.now-row {
  display: flex;
  align-items: baseline;
  gap: 8px;
  flex-wrap: wrap;
}
.now-k {
  min-width: 72px;
  font-size: 12.5px;
  font-weight: 600;
  color: var(--color-text);
}
.now-v {
  font-size: 12.5px;
  font-weight: 700;
  color: var(--color-text-2);
}
.now-v.ok {
  color: var(--color-ok);
}
.now-v.dim {
  color: var(--color-text-3);
  font-weight: 600;
}
.now-note {
  font-size: 11.5px;
  color: var(--color-text-3);
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
  color: var(--color-primary-text);
}
.btn:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--color-primary) 55%, transparent);
  outline-offset: 2px;
}
</style>
