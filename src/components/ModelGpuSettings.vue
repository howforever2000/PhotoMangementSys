<script setup lang="ts">
/**
 * 识别性能设置（FEAT-051）—— 原子组件
 *
 * 交互流程（参考单相册扫描面板「检测 GPU」模式，按用户定义）：
 *   1. 先选分类模型（默认按候选梯度回退，当前生效实时展示）
 *   2. 默认 CPU 推理；「🔍 检测 GPU」探测可用加速提供方
 *   3. 检测到可用 GPU → 「🚀 启用加速」；未检测到 → 提示安装 GPU 版运行时
 *
 * 自包含：挂载即拉取状态（内部自动拉起识别微服务）；切换即时保存并刷新展示。
 * 复用方：GlobalScanPanel（全局照片扫描入库）；ScanPanel 等其他扫描入口可直接内嵌。
 */
import { computed, onMounted, ref } from "vue";
import { useContentStore } from "../stores/content";
import { useNotify } from "../composables/useNotify";
import type { VcrModelsInfo } from "../types/content";

const contentStore = useContentStore();
const notify = useNotify();

const initFailed = ref(false);
const detectBusy = ref(false);
const accelBusy = ref(false);
const modelBusy = ref(false);
const modelsInfo = ref<VcrModelsInfo | null>(null);
const selectedModel = ref("");
/** 是否已执行过「检测 GPU」 */
const detected = ref(false);

const gpu = computed(() => contentStore.gpuStatus);
/** 检测到真正可用的本地 GPU 提供方（DirectML/CUDA 等，排除 Azure 云端） */
const gpuAvailable = computed(() => (gpu.value?.gpu.length ?? 0) > 0);
/** 当前是否已在 GPU 上推理 */
const accelerating = computed(() => gpu.value?.use_gpu === true);

async function refreshAll(silent = false) {
  const [gpuRes, modelsRes] = await Promise.allSettled([
    contentStore.fetchGpuStatus(),
    contentStore.fetchVcrModels(),
  ]);
  if (modelsRes.status === "fulfilled") {
    modelsInfo.value = modelsRes.value;
    selectedModel.value = modelsRes.value.current ?? "";
  }
  detected.value = gpuRes.status === "fulfilled";
  if (gpuRes.status === "rejected" && modelsRes.status === "rejected") {
    if (!silent) throw gpuRes.reason;
    initFailed.value = true;
  }
}

onMounted(async () => {
  try {
    await refreshAll(true);
  } catch {
    initFailed.value = true;
  }
});

/** 检测 GPU：刷新状态并提示可用性 */
async function detect() {
  detectBusy.value = true;
  try {
    await refreshAll(true);
    detected.value = true;
    if (gpuAvailable.value) {
      notify.success("检测到可用 GPU 加速", gpu.value!.gpu.join("、"));
    } else {
      notify.info(
        "未检测到可用 GPU",
        "当前使用 CPU 推理；可安装 onnxruntime-directml（或 CUDA 版）后重新检测",
      );
    }
  } catch (e) {
    notify.error("检测失败", String(e));
  } finally {
    detectBusy.value = false;
  }
}

/** 启用 / 关闭加速 */
async function toggleAccel() {
  accelBusy.value = true;
  const target = !accelerating.value;
  try {
    const s = await contentStore.setVcrGpu(target);
    notify.success(
      target ? "已启用 GPU 加速" : "已关闭加速，使用 CPU 推理",
      s.provider,
    );
  } catch (e) {
    notify.error("切换失败", String(e));
  } finally {
    accelBusy.value = false;
  }
}

/** 模型切换（失败自动回显原值） */
async function onModelChange() {
  const name = selectedModel.value;
  modelBusy.value = true;
  try {
    const info = await contentStore.setVcrModel(name);
    modelsInfo.value = info;
    selectedModel.value = info.current ?? name;
    notify.success("分类模型已切换", `${name} · 影响后续扫描`);
  } catch (e) {
    notify.error("切换模型失败", String(e));
    try {
      const info = await contentStore.fetchVcrModels();
      modelsInfo.value = info;
      selectedModel.value = info.current ?? "";
    } catch {
      /* 回显失败忽略 */
    }
  } finally {
    modelBusy.value = false;
  }
}
</script>

<template>
  <div class="mgps-wrap">
    <div class="mgps-head">⚙ 识别性能设置</div>

    <!-- 微服务不可用降级提示 -->
    <p v-if="initFailed" class="mgps-hint mgps-hint-warn">
      识别服务暂不可用（扫描启动时会自动拉起，届时可在此调整）。
    </p>

    <template v-else>
      <!-- 1. 先选模型 -->
      <div class="mgps-row">
        <span class="mgps-label">分类模型</span>
        <select
          v-model="selectedModel"
          class="mgps-select"
          :disabled="modelBusy || !modelsInfo"
          @change="onModelChange"
        >
          <option
            v-for="m in modelsInfo?.models ?? []"
            :key="m.name"
            :value="m.name"
            :disabled="!m.downloaded"
          >
            {{ m.label }}（精度 {{ m.accuracy }} · {{ m.speed }}）{{ m.downloaded ? "" : " —— 未下载" }}{{ m.active ? " ✓当前" : "" }}
          </option>
        </select>
        <span v-if="modelBusy" class="mgps-busy">切换中…</span>
      </div>

      <!-- 2. 检测 GPU -->
      <div class="mgps-row">
        <span class="mgps-label">运行硬件</span>
        <button class="mgps-btn" :disabled="detectBusy" @click="detect">
          {{ detectBusy ? "检测中…" : "🔍 检测 GPU" }}
        </button>
        <template v-if="detected && gpu">
          <span v-if="gpuAvailable" class="mgps-status ok">
            可用加速：{{ gpu.gpu.join("、") }}
          </span>
          <span v-else class="mgps-status">
            未检测到可用 GPU（可安装 onnxruntime-directml 后重新检测）
          </span>
        </template>
        <span v-else class="mgps-status dim">默认使用 CPU 推理</span>
      </div>

      <!-- 3. 启用加速（检测到可用 GPU 后开放） -->
      <div class="mgps-row">
        <span class="mgps-label">GPU 加速</span>
        <button
          class="mgps-accel"
          :class="{ on: accelerating }"
          :disabled="!gpuAvailable || accelBusy"
          :title="gpuAvailable ? '' : '请先「检测 GPU」确认存在可用加速提供方'"
          @click="toggleAccel"
        >
          {{ accelBusy ? "切换中…" : accelerating ? "✅ 加速中 · 点击关闭" : "🚀 启用加速" }}
        </button>
        <span class="mgps-status" :class="{ ok: accelerating }">
          {{ gpu ? (accelerating ? `GPU（${gpu.provider}）` : "CPU 推理（默认）") : "" }}
        </span>
      </div>

      <p class="mgps-hint">
        默认使用 CPU 推理；检测到 GPU 后可开启加速（需 GPU 版运行时，如
        <code>onnxruntime-directml</code>）。切换<b>即时生效</b>，影响后续扫描；
        新模型下载后放入 <code>python/models/</code>（如 yolov8x-cls.onnx）即可在此选择。
      </p>
    </template>
  </div>
</template>

<style scoped>
.mgps-wrap {
  border: 1px solid rgba(127, 127, 127, 0.25);
  border-radius: 12px;
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.mgps-head {
  font-size: 13.5px;
  font-weight: 700;
  opacity: 0.85;
}
.mgps-row {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}
.mgps-label {
  font-size: 13px;
  font-weight: 600;
  min-width: 70px;
}
.mgps-btn {
  padding: 5px 14px;
  border-radius: 8px;
  border: 1px solid rgba(127, 127, 127, 0.4);
  background: transparent;
  color: inherit;
  font-size: 12.5px;
  cursor: pointer;
  transition: border-color 0.15s, background 0.15s;
}
.mgps-btn:hover:not(:disabled) {
  border-color: rgba(106, 141, 240, 0.75);
  background: rgba(106, 141, 240, 0.08);
}
.mgps-btn:disabled {
  opacity: 0.55;
  cursor: wait;
}

.mgps-accel {
  padding: 5px 14px;
  border-radius: 8px;
  border: 1px solid rgba(127, 127, 127, 0.4);
  background: transparent;
  color: inherit;
  font-size: 12.5px;
  cursor: not-allowed;
  transition: border-color 0.15s, background 0.15s;
}
.mgps-accel:disabled {
  opacity: 0.5;
}
.mgps-accel:not(:disabled) {
  cursor: pointer;
  border-color: rgba(106, 141, 240, 0.75);
}
.mgps-accel:not(:disabled):hover {
  background: rgba(106, 141, 240, 0.1);
}
.mgps-accel.on {
  background: #396cd8;
  border-color: #396cd8;
  color: #fff;
  cursor: pointer;
}

.mgps-select {
  flex: 1;
  min-width: 260px;
  max-width: 460px;
  padding: 6px 10px;
  border-radius: 8px;
  border: 1px solid rgba(127, 127, 127, 0.4);
  background: transparent;
  color: inherit;
  font-size: 12.5px;
  cursor: pointer;
}
.mgps-select:disabled {
  opacity: 0.55;
  cursor: wait;
}
.mgps-busy {
  font-size: 12px;
  opacity: 0.65;
}
.mgps-status {
  font-size: 12.5px;
  color: #a1642a;
}
.mgps-status.ok {
  color: #15803d;
}
.mgps-status.dim {
  opacity: 0.6;
}

.mgps-hint {
  font-size: 11.5px;
  opacity: 0.65;
  line-height: 1.6;
  margin: 0;
}
.mgps-hint code {
  padding: 1px 5px;
  border-radius: 4px;
  background: rgba(127, 127, 127, 0.18);
  font-size: 11px;
}
.mgps-hint-warn {
  color: #b45309;
  opacity: 1;
}
</style>
