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
import { computed, onMounted, ref, watch } from "vue";
import { useContentStore } from "../stores/content";
import { useNotify } from "../composables/useNotify";
import type { ModelDlStatus, VcrModelsInfo } from "../types/content";

const contentStore = useContentStore();
const notify = useNotify();

const initFailed = ref(false);
const detectBusy = ref(false);
const accelBusy = ref(false);
const modelBusy = ref(false);
const modelsInfo = ref<VcrModelsInfo | null>(null);
const selectedModel = ref("");
/** 模型清单加载失败（识别服务升级中/异常）→ 提示而非无限转圈 */
const modelsFailed = ref(false);
/** FEAT-052：模型下载状态（store 由 model-dl-progress 事件实时更新） */
const downloads = computed(() => contentStore.modelDownloads);
/** 是否已执行过「检测 GPU」 */
const detected = ref(false);

const gpu = computed(() => contentStore.gpuStatus);
/** 检测到真正可用的本地 GPU 提供方（DirectML/CUDA 等，排除 Azure 云端） */
const gpuAvailable = computed(() => (gpu.value?.gpu.length ?? 0) > 0);
/** 当前是否已在 GPU 上推理 */
const accelerating = computed(() => gpu.value?.use_gpu === true);

async function refreshAll(silent = false) {
  modelsFailed.value = false;
  const [gpuRes, modelsRes] = await Promise.allSettled([
    contentStore.fetchGpuStatus(),
    contentStore.fetchVcrModels(),
  ]);
  if (modelsRes.status === "fulfilled") {
    modelsInfo.value = modelsRes.value;
    selectedModel.value = modelsRes.value.current ?? "";
  } else {
    modelsFailed.value = true;
  }
  detected.value = gpuRes.status === "fulfilled";
  if (gpuRes.status === "rejected" && modelsRes.status === "rejected") {
    if (!silent) throw gpuRes.reason;
    initFailed.value = true;
  }
}

onMounted(async () => {
  try {
    await Promise.allSettled([refreshAll(true), contentStore.listModelDownloads()]);
  } catch {
    initFailed.value = true;
  }
});

/** 服务不可用（启动/加载模型中）→ 手动重试拉起状态；就绪后恢复面板 */
async function retryInit() {
  detectBusy.value = true;
  try {
    await refreshAll(false); // 全部失败时抛错
    initFailed.value = false;
  } catch {
    initFailed.value = true;
  } finally {
    detectBusy.value = false;
  }
}

/** 任一模型刚完成下载 → 刷新候选清单与当前生效（新模型立即可选） */
const prevDone = ref(new Set<string>());
watch(
  () => contentStore.modelDownloads,
  (list) => {
    for (const d of list) {
      if (d.done && !prevDone.value.has(d.name)) {
        prevDone.value.add(d.name);
        contentStore
          .fetchVcrModels()
          .then((info) => {
            modelsInfo.value = info;
            selectedModel.value = info.current ?? "";
          })
          .catch(() => {});
      }
      if (!d.done) prevDone.value.delete(d.name);
    }
  },
  { deep: true },
);

function progressPct(d: ModelDlStatus): number {
  if (d.stage === "exporting") return 100;
  if (!d.total) return 0;
  return Math.min(100, Math.round((d.bytes / d.total) * 100));
}
function fmtBytes(d: ModelDlStatus): string {
  const b = (d.bytes / 1e6).toFixed(1);
  const t = d.total ? ` / ${(d.total / 1e6).toFixed(1)} MB` : "";
  return `${b} MB${t}`;
}
function startDl(name: string) {
  contentStore.startModelDownload(name).catch((e) => notify.error("启动下载失败", String(e)));
}
function cancelDl(name: string) {
  contentStore.cancelModelDownload(name).catch((e) => notify.error("取消失败", String(e)));
}

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

/** 模型切换（失败自动回显原值；切换为后台加载，就绪前 /health 由宿主等待） */
async function onModelChange() {
  const name = selectedModel.value;
  modelBusy.value = true;
  try {
    const info = await contentStore.setVcrModel(name);
    modelsInfo.value = info;
    selectedModel.value = info.current ?? name;
    notify.success(
      "分类模型已切换",
      info.cls_ready === false
        ? `${name} · 后台加载中，就绪后自动生效`
        : `${name} · 影响后续扫描`,
    );
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

    <!-- 微服务不可用降级提示（服务启动/加载模型期间可重试，就绪后自动恢复） -->
    <p v-if="initFailed" class="mgps-hint mgps-hint-warn">
      识别服务暂不可用（可能正在启动/加载模型，稍候重试）。
      <button class="mgps-btn" :disabled="detectBusy" @click="retryInit">
        {{ detectBusy ? "重试中…" : "🔄 重试" }}
      </button>
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
        <span v-else-if="modelsInfo?.cls_ready === false" class="mgps-busy">模型加载中…</span>
        <span v-else-if="modelsFailed" class="mgps-status">模型清单加载失败（识别服务可能正在升级，稍后重试）</span>
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

      <!-- FEAT-052：模型下载（后台 + 进度 + 官方/镜像择优） -->
      <div class="mgps-row mgps-dl-head">
        <span class="mgps-label">📥 模型下载</span>
        <span class="mgps-hint">官方 / 镜像并行择快；.pt 下载后自动导出 onnx</span>
      </div>
      <div class="mgps-dl-list">
        <div v-for="d in downloads" :key="d.name" class="mgps-dl-row">
          <span class="mgps-dl-name">
            {{ d.name }}<span v-if="d.required" class="mgps-tag-req">必需</span>
          </span>
          <template v-if="d.done">
            <span class="mgps-status ok">✓ 已下载</span>
          </template>
          <template v-else-if="d.running">
            <div class="mgps-progress">
              <div class="mgps-progress-fill" :style="{ width: progressPct(d) + '%' }"></div>
            </div>
            <span class="mgps-busy">
              {{ d.stage === "exporting" ? "导出中…" : fmtBytes(d) }}
            </span>
            <button class="mgps-btn mgps-btn-sm" @click="cancelDl(d.name)">取消</button>
          </template>
          <template v-else-if="d.stage === 'error'">
            <span class="mgps-status err" :title="d.error ?? ''">
              失败：{{ (d.error ?? "").slice(0, 40) }}
            </span>
            <button class="mgps-btn mgps-btn-sm" @click="startDl(d.name)">重试</button>
          </template>
          <template v-else>
            <button class="mgps-btn mgps-btn-sm" @click="startDl(d.name)">⬇ 下载</button>
          </template>
        </div>
      </div>
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

/* FEAT-052：模型下载 */
.mgps-dl-head {
  margin-top: 4px;
}
.mgps-dl-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.mgps-dl-row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  padding: 4px 0;
  border-bottom: 1px dashed rgba(127, 127, 127, 0.2);
}
.mgps-dl-row:last-child {
  border-bottom: none;
}
.mgps-dl-name {
  font-size: 12.5px;
  font-weight: 600;
  min-width: 190px;
}
.mgps-tag-req {
  margin-left: 6px;
  padding: 1px 6px;
  font-size: 10.5px;
  color: #b45309;
  border: 1px solid rgba(180, 83, 9, 0.4);
  border-radius: 999px;
}
.mgps-progress {
  flex: 1;
  min-width: 140px;
  max-width: 260px;
  height: 8px;
  border-radius: 999px;
  background: rgba(127, 127, 127, 0.25);
  overflow: hidden;
}
.mgps-progress-fill {
  height: 100%;
  border-radius: 999px;
  background: linear-gradient(90deg, #396cd8, #7eb6ff);
  transition: width 0.2s;
}
.mgps-status.err {
  color: #e03131;
}
.mgps-btn-sm {
  padding: 3px 10px;
  font-size: 11.5px;
}
</style>
