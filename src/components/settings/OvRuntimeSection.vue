<script setup lang="ts">
/**
 * 语义索引加速（OpenVINO）—— 只作用于「语义图像塔」这一条腿
 *
 * 为什么单开一个分区而不是塞进「GPU 加速」：两者接管的是**不同通道**。
 *   GPU 加速（DirectML）→ 人物检测 / 人脸 / OCR
 *   OpenVINO（NPU）    → 语义索引（把照片编成向量）
 * 混在一起是这一屏原来「看不懂」的主要来源。
 *
 * 三条产品纪律（写在这里免得后人改坏）：
 *   1. **只在 Intel 平台推荐**——OpenVINO 的 GPU/NPU 插件只认 Intel，AMD/NVIDIA
 *      机器装了没有任何收益，所以那两类机器上直接显示「不适用」而不是给个按钮。
 *   2. **组件是可选的**：没装就完全走原来的 ONNX Runtime，行为与 FEAT-133 之前逐字一致。
 *   3. **永远给一条退路**：`ort` 模式一键回到原后端，不需要卸载组件（驱动/数值可疑时用）。
 *
 * 数字口径：本机实测 8 张一批（Intel Ultra 5 245K / AI Boost NPU，fp32 档）
 *   CPU 50.8ms/张 → NPU 19.3ms/张（约 2.6×）；单张散跑时 NPU 约 29ms（固定开销约 10ms）。
 * 已开启 GPU 加速的机器提升幅度会小一些，所以文案不承诺倍数。
 */
import { computed, onMounted, ref } from "vue";
import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import { PERF_TIMEOUT, withTimeout } from "../../utils/withTimeout";

defineProps<{ locked?: boolean }>();

const store = useContentStore();
const notify = useNotify();

const RUNTIME_MODEL = "openvino-runtime";

const rt = computed(() => store.clipRuntime);
const info = computed(() => rt.value?.runtime);
const busy = ref(false);
const loading = ref(false);
const failed = ref(false);

/** 组件下载状态（复用 model_dl 的进度通道） */
const dl = computed(() => store.modelDownloads.find((d) => d.name === RUNTIME_MODEL) ?? null);
const downloading = computed(() => dl.value?.running === true);
const installed = computed(() => info.value?.installed === true);
const usable = computed(() => info.value?.usable === true);
const isIntel = computed(() => info.value?.vendor === "intel");
const isAmd = computed(() => info.value?.vendor === "amd");
const mode = computed(() => rt.value?.mode ?? "auto");
const active = computed(() => rt.value?.active === true);
const deviceLabel = computed(() => {
  const d = rt.value?.device ?? "";
  if (d === "NPU") return "NPU";
  if (d === "GPU") return "核显";
  if (d === "CPU") return "CPU";
  return "";
});

async function load(silent = true) {
  loading.value = true;
  failed.value = false;
  try {
    await withTimeout(store.fetchClipRuntime(), PERF_TIMEOUT.read, "get_vcr_clip_runtime");
    await store.listModelDownloads();
    return true;
  } catch (e) {
    failed.value = true;
    if (!silent) throw e;
    return false;
  } finally {
    loading.value = false;
  }
}

onMounted(() => void load(true));

async function download() {
  busy.value = true;
  try {
    await store.startModelDownload(RUNTIME_MODEL);
    notify.success("开始下载", "约 15MB，完成后语义索引会自动切到 NPU");
  } catch (e) {
    notify.error("下载失败", String(e));
  } finally {
    busy.value = false;
  }
}

async function toggleMode() {
  busy.value = true;
  const next: "auto" | "ort" = mode.value === "auto" ? "ort" : "auto";
  try {
    const s = await withTimeout(store.setClipRuntime(next), PERF_TIMEOUT.write, "set_vcr_clip_runtime");
    notify.success(
      next === "auto" ? "语义索引已恢复自动选择" : "语义索引已切回 ONNX Runtime",
      next === "auto" ? `当前后端：${s.backend}${s.device ? " · " + s.device : ""}` : "与未装组件时完全一致",
    );
  } catch (e) {
    notify.error("切换失败", String(e));
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="ov-head">
    <span class="lbl">语义索引加速</span>
    <span v-if="isIntel" class="badge intel">Intel 平台</span>
    <span v-else-if="isAmd" class="badge dim">AMD 平台 · 不适用</span>
    <span v-else class="badge dim">未识别 CPU 厂商</span>

    <button v-if="failed" class="btn btn-sm" :disabled="locked || loading" @click="load(false)">
      重试
    </button>
  </div>

  <!-- Intel 且未装组件：给下载入口 -->
  <template v-if="isIntel && !installed">
    <p class="hint">
      装一个 <b>40MB</b> 的运行时（不随安装包下发），照片编向量就能从 CPU 挪到
      <b>NPU</b>：实测 8 张一批 <b>50.8 → 19.3 ms/张</b>。
      不装也完全正常，只是语义索引继续用 CPU。
    </p>
    <div class="row">
      <button class="btn btn-primary" :disabled="locked || busy || downloading" @click="download">
        {{ downloading ? "下载中…" : busy ? "启动中…" : "⬇ 下载 OpenVINO 运行时（约 15MB 下载 / 40MB 装后）" }}
      </button>
      <span v-if="dl?.error" class="err">{{ dl.error }}</span>
    </div>
    <div v-if="downloading && dl" class="bar" role="progressbar" :aria-valuenow="dl.bytes" :aria-valuemax="dl.total">
      <i :style="{ width: `${Math.min(100, dl.total ? (dl.bytes / dl.total) * 100 : 0)}%` }"></i>
    </div>
  </template>

  <!-- Intel 且已装：显示实际设备 + 后端开关 -->
  <template v-else-if="isIntel && installed">
    <p class="hint">
      <template v-if="usable && active">
        语义索引正跑在 <b>{{ deviceLabel || info?.preferred_device }}</b> 上
        <span v-if="rt?.load_ms">（首次编译 {{ (rt.load_ms / 1000).toFixed(1) }}s，已缓存）</span>。
        人物检测 / 人脸 / OCR <b>不受影响</b>，它们走「GPU 加速」那条线。
      </template>
      <template v-else-if="usable && mode === 'ort'">
        组件已装，但你选择了 <b>ONNX Runtime</b> 后端 —— 与未装组件时行为完全一致。
      </template>
      <template v-else-if="info?.error">
        组件已装但不可用：<span class="err">{{ info.error }}</span>
      </template>
      <template v-else-if="!info?.devices?.length">
        组件已装，但没探测到 <b>NPU / 核显</b> 设备（老 Intel 机器常见），语义索引继续用 CPU 即可。
      </template>
    </p>
    <div class="row">
      <button
        v-if="usable"
        class="btn"
        :class="{ on: mode === 'auto' && active }"
        :disabled="locked || busy"
        @click="toggleMode"
      >
        {{ busy ? "切换中…" : mode === "auto" ? "✅ 已启用 · 点击切回 CPU 后端" : "🚀 启用 NPU 加速" }}
      </button>
      <span v-if="info?.npu" class="badge ok">NPU 就绪</span>
      <span v-else-if="info?.igpu" class="badge">核显就绪</span>
      <span v-if="info?.version" class="ver">运行时 {{ info.version.split("-")[0] }}</span>
    </div>
  </template>

  <!-- 非 Intel 或无组件信息：明确说"不适用/无收益"，不给按钮 -->
  <template v-else>
    <p class="hint">
      该加速依赖 Intel 的 OpenVINO 运行时，<b>只对 Intel CPU 的核显 / NPU 有效</b>。
      这台机器上不适用 —— 语义索引继续用 CPU（或你已开启的 GPU 加速），不影响任何功能。
    </p>
  </template>
</template>

<style scoped>
.ov-head {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.lbl {
  min-width: 96px;
  font-size: 12.5px;
  color: var(--color-text-2);
}
.row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
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
  transition: background var(--dur-2), border-color var(--dur-2), color var(--dur-2);
}
.btn:hover:not(:disabled) {
  border-color: var(--color-primary);
  color: var(--color-primary-text);
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
  color: var(--color-primary-text);
}
.btn.on {
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
  padding: 3px 9px;
  border-radius: 999px;
  background: var(--color-neutral-soft);
  color: var(--color-text-2);
  font-size: 12px;
}
.badge.intel {
  background: var(--color-primary-soft);
  color: var(--color-primary-text);
}
.badge.ok {
  background: var(--color-ok-soft);
  color: var(--color-ok);
  font-weight: 600;
}
.badge.dim {
  color: var(--color-text-3);
}
.ver {
  font-size: 11.5px;
  color: var(--color-text-3);
}
.hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--color-text-3);
}
.err {
  font-size: 12px;
  color: var(--color-danger);
}
.bar {
  height: 6px;
  border-radius: 999px;
  background: var(--color-neutral-soft);
  overflow: hidden;
}
.bar > i {
  display: block;
  height: 100%;
  background: var(--color-primary);
  transition: width var(--dur-2);
}
</style>
