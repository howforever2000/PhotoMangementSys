<script setup lang="ts">
/**
 * 识别性能设置（FEAT-051）—— 原子组件
 *
 * GPU 加速开关 + 分类模型选择（x/l/m/s/n 梯度，缺失置灰提示下载）。
 * 自包含：挂载即拉取状态（内部自动拉起识别微服务）；切换即时保存并刷新展示。
 *
 * 复用方：GlobalScanPanel（全局照片扫描入库）；ScanPanel 等其他扫描入口可直接内嵌。
 * 交互友好：
 *  - 开关为 iOS 风格 pill，右侧实时显示当前 provider（✅ GPU / CPU）
 *  - 模型下拉每项标注准确率 / 速度 / 未下载 / 当前，切换失败自动回显原值
 *  - 切换中 loading 态；成功 toast 提示影响范围
 */
import { computed, onMounted, ref } from "vue";
import { useContentStore } from "../stores/content";
import { useNotify } from "../composables/useNotify";
import type { VcrModelsInfo } from "../types/content";

const contentStore = useContentStore();
const notify = useNotify();

const gpuBusy = ref(false);
const modelBusy = ref(false);
const modelsInfo = ref<VcrModelsInfo | null>(null);
const selectedModel = ref("");
/** 开关状态（默认开 = GPU 优先；后端 forced_cpu 时为关） */
const gpuOn = ref(true);
/** 初始化失败（微服务不可用）——展示降级提示 */
const initFailed = ref(false);

const gpu = computed(() => contentStore.gpuStatus);

async function refresh() {
  const [gpuRes, modelsRes] = await Promise.allSettled([
    contentStore.fetchGpuStatus(),
    contentStore.fetchVcrModels(),
  ]);
  if (gpuRes.status === "fulfilled") gpuOn.value = !gpuRes.value.forced_cpu;
  if (modelsRes.status === "fulfilled") {
    modelsInfo.value = modelsRes.value;
    selectedModel.value = modelsRes.value.current ?? "";
  }
  if (gpuRes.status === "rejected" && modelsRes.status === "rejected") {
    throw gpuRes.reason;
  }
}

onMounted(async () => {
  try {
    await refresh();
  } catch {
    initFailed.value = true;
  }
});

/** GPU 开关切换（开 = 自动 GPU 优先 / 关 = 强制 CPU） */
async function toggleGpu() {
  gpuBusy.value = true;
  const prev = gpuOn.value;
  try {
    const s = await contentStore.setVcrGpu(!prev);
    gpuOn.value = !s.forced_cpu;
    notify.success(
      s.use_gpu ? "已启用 GPU 加速" : "已切换为 CPU 推理",
      s.provider,
    );
  } catch (e) {
    gpuOn.value = prev;
    notify.error("切换 GPU 失败", String(e));
  } finally {
    gpuBusy.value = false;
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
      <!-- GPU 加速开关 -->
      <div class="mgps-row">
        <span class="mgps-label">GPU 加速</span>
        <button
          class="mgps-toggle"
          :class="{ on: gpuOn, busy: gpuBusy }"
          :disabled="gpuBusy"
          role="switch"
          :aria-checked="gpuOn"
          :title="gpuOn ? '点击关闭（强制 CPU 推理）' : '点击开启（DirectML / CUDA 优先）'"
          @click="toggleGpu"
        >
          <span class="mgps-knob"></span>
          <span class="mgps-toggle-text">{{ gpuBusy ? "切换中…" : gpuOn ? "开" : "关" }}</span>
        </button>
        <span class="mgps-status" :class="{ ok: gpu?.use_gpu }">
          <template v-if="gpu">
            {{ gpu.use_gpu ? `✅ GPU（${gpu.provider}）` : `CPU（${gpu.provider}）` }}
          </template>
          <template v-else>检测中…</template>
        </span>
      </div>

      <!-- 分类模型选择 -->
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

      <p class="mgps-hint">
        切换<b>即时生效</b>，影响后续扫描（当前批次不受影响）。更大的模型更准但更慢：
        <b>x</b> 建议 GPU 用户、<b>l</b> 适合 7840HS 级 CPU。
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
  margin-bottom: 16px;
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

/* iOS 风格 pill 开关 */
.mgps-toggle {
  position: relative;
  width: 62px;
  height: 26px;
  border-radius: 999px;
  border: none;
  cursor: pointer;
  background: rgba(127, 127, 127, 0.35);
  transition: background 0.18s;
  display: flex;
  align-items: center;
  padding: 0 8px 0 30px;
}
.mgps-toggle.on {
  background: #396cd8;
  padding: 0 30px 0 8px;
  justify-content: flex-end;
}
.mgps-toggle.busy {
  opacity: 0.6;
  cursor: wait;
}
.mgps-knob {
  position: absolute;
  left: 3px;
  width: 20px;
  height: 20px;
  border-radius: 50%;
  background: #fff;
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.3);
  transition: left 0.18s;
}
.mgps-toggle.on .mgps-knob {
  left: calc(100% - 23px);
}
.mgps-toggle-text {
  color: #fff;
  font-size: 12px;
  font-weight: 600;
  user-select: none;
}

.mgps-status {
  font-size: 12.5px;
  color: #a1642a;
}
.mgps-status.ok {
  color: #15803d;
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
