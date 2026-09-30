<script setup lang="ts">
/**
 * ① 本机硬件 · GPU 加速（运行硬件探测 + 加速开关）
 *
 * 数据直接读 Pinia `gpuStatus`：动作也在本组件内完成，其他分区读同一份 store，
 * 因此「在弹窗里开关加速 → 扫描面板的 GPU 徽章立刻同步」不需要任何 prop 传递。
 *
 * 实测口径（bench_cpu_gpu_modes，P=6、40 张，点 4 串行锁后）：
 *   cpu-p6 = 109.4 ms/张 · gpu-p6 = 80.0~82.6 ms/张 ⇒ GPU 反超 1.33~1.37×
 * （点 4 之前 GPU 模式 P≥2 会段错误，旧版「GPU 判负」的结论已不成立。）
 */
import { computed, onMounted, ref } from "vue";
import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import { PERF_TIMEOUT, withTimeout } from "../../utils/withTimeout";

defineProps<{ locked?: boolean }>();

const store = useContentStore();
const notify = useNotify();

const gpu = computed(() => store.gpuStatus);
const detectBusy = ref(false);
const accelBusy = ref(false);
const gpuLoading = ref(false);
const gpuFailed = ref(false);
/** 是否执行过一次检测（未检测前不渲染成「没有 GPU」） */
const detected = ref(false);

const gpuAvailable = computed(() => (gpu.value?.gpu.length ?? 0) > 0);
const accelerating = computed(() => gpu.value?.use_gpu === true);

async function loadGpu(silent = true) {
  gpuLoading.value = true;
  gpuFailed.value = false;
  try {
    await withTimeout(store.fetchGpuStatus(), PERF_TIMEOUT.read, "get_vcr_gpu_status");
    detected.value = true;
    return true;
  } catch (e) {
    gpuFailed.value = true;
    if (!silent) throw e;
    return false;
  } finally {
    gpuLoading.value = false;
  }
}

/** 行内重试：只重拉 GPU 状态，不牵连其他分区（BUG-2026-0921-003） */
function retryGpu() {
  void loadGpu(true);
}

onMounted(() => void loadGpu(true));

async function detect() {
  detectBusy.value = true;
  try {
    await loadGpu(false);
    if (gpuAvailable.value) {
      notify.success("检测到可用 GPU 加速", (gpu.value?.gpu ?? []).join("、"));
    } else {
      notify.info(
        "未检测到可用 GPU",
        "当前使用 CPU 推理。发布版已内置 DirectML 运行时，检测不到通常是：显卡驱动过旧 / 不支持 DirectX 12 / 虚拟机环境。更新显卡驱动后重新检测即可。",
      );
    }
  } catch (e) {
    notify.error("检测失败", String(e));
  } finally {
    detectBusy.value = false;
  }
}

async function toggleAccel() {
  accelBusy.value = true;
  const target = !accelerating.value;
  try {
    const s = await withTimeout(store.setVcrGpu(target), PERF_TIMEOUT.write, "set_vcr_gpu");
    notify.success(target ? "已启用 GPU 加速" : "已关闭加速，使用 CPU 推理", s.provider);
  } catch (e) {
    notify.error("切换失败", String(e));
  } finally {
    accelBusy.value = false;
  }
}

/** provider 短名（Dml → DirectML，便于一眼看出走没走 GPU） */
function providerLabel(p: string): string {
  if (p.startsWith("Dml")) return "DirectML";
  if (p.startsWith("Cuda")) return "CUDA";
  return p.replace("ExecutionProvider", "");
}
</script>

<template>
  <div class="sec-row">
    <span class="lbl">GPU 加速</span>
    <button
      class="btn btn-primary"
      :class="{ on: accelerating }"
      :disabled="locked || !gpuAvailable || accelBusy || detectBusy || gpuLoading"
      :title="gpuAvailable ? '点击关闭加速，回到 CPU 推理' : '请先「检测 GPU」确认存在可用加速提供方'"
      @click="toggleAccel"
    >
      {{ accelBusy ? "切换中…" : accelerating ? "✅ 加速中 · 点击关闭" : "🚀 启用加速" }}
    </button>
    <button class="btn" :disabled="locked || detectBusy || gpuLoading" @click="detect">
      {{ detectBusy || gpuLoading ? "检测中…" : "🔍 检测 GPU" }}
    </button>

    <!-- 状态徽章：一眼看出当前跑在哪 -->
    <span v-if="accelerating" class="badge ok" :title="gpu ? `当前 provider：${gpu.provider}` : ''">
      {{ providerLabel(gpu?.provider ?? "") }} 加速
    </span>
    <span v-else-if="gpuFailed" class="badge err">
      状态获取失败
      <button class="btn btn-sm" @click="retryGpu">重试</button>
    </span>
    <span v-else-if="detected && gpuAvailable" class="badge">
      可用：{{ (gpu?.gpu ?? []).map(providerLabel).join(" / ") }}
    </span>
    <span v-else class="badge dim">CPU 推理（默认）</span>
  </div>
  <p class="hint">
    实测（8 核 16 线程 · 40 张）：GPU <b>80.0~82.6</b> vs CPU <b>109.4</b> ms/张，识别约快 1.3×。
    加速只作用于<b>人物检测 / 人脸 / OCR</b>，语义模型默认仍走 CPU。
  </p>
</template>

<style scoped>
.sec-row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.lbl {
  min-width: 76px;
  font-size: 12.5px;
  color: var(--color-text-2);
}
.btn {
  min-height: 32px;
  padding: 6px 12px;
  border-radius: var(--radius-sm, 8px);
  border: 1px solid var(--color-border);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s, color 0.15s;
}
.btn:hover:not(:disabled) {
  border-color: var(--color-primary);
  color: var(--color-primary);
}
.btn:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--color-primary) 55%, transparent);
  outline-offset: 2px;
}
.btn:disabled {
  background: var(--color-neutral-soft);
  color: var(--color-text-3);
  cursor: not-allowed;
}
.btn-primary {
  background: var(--color-primary-soft);
  border-color: #b9cdf5;
  color: var(--color-primary);
}
.btn-primary.on {
  background: var(--color-ok-soft);
  border-color: #a7d9b5;
  color: var(--color-ok);
}
.btn-sm {
  min-height: 24px;
  padding: 2px 8px;
  font-size: 11.5px;
}
.badge {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 3px 9px;
  border-radius: 999px;
  background: var(--color-neutral-soft);
  color: var(--color-text-2);
  font-size: 12px;
}
.badge.ok {
  background: var(--color-ok-soft);
  color: var(--color-ok);
  font-weight: 600;
}
.badge.err {
  background: var(--color-danger-soft);
  color: var(--color-danger);
}
.badge.dim {
  color: var(--color-text-3);
}
.hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--color-text-3);
}
</style>
