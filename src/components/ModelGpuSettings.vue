<script setup lang="ts">
/**
 * 识别性能设置（FEAT-051 / v5）—— 原子组件
 *
 * 交互流程（与旧「分类模型」下拉同构，只是换成语义模型档位）：
 *   1. 先选**语义模型档位**（Chinese-CLIP B/16 ↔ L/14-336；适配不同硬件，
 *      缺档位置灰并可直接下载；切换后需重建语义索引）
 *   2. 默认 CPU 推理；「🔍 检测 GPU」探测可用加速提供方（对人物检测/OCR 生效）
 *   3. 检测到可用 GPU → 「🚀 启用加速」；未检测到 → 提示安装 GPU 版运行时
 *
 * 自包含：挂载即拉取状态（内部自动拉起识别微服务）；切换即时保存并刷新展示。
 * 复用方：GlobalScanPanel（全局照片扫描入库）；ScanPanel 等其他扫描入口可直接内嵌。
 */
import { computed, onMounted, ref, watch } from "vue";
import { useContentStore } from "../stores/content";
import { useNotify } from "../composables/useNotify";
import { PERF_TIMEOUT, withTimeout } from "../utils/withTimeout";
import type {
  ModelDlStatus,
  ModelSourceProbe,
  VcrBenchmarkResult,
  VcrModelsInfo,
  VcrSweepEntry,
  VcrThreadsInfo,
} from "../types/content";

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
// 三个数据源各自独立：任一慢/错都不拖垮其余下拉与按钮（BUG-2026-0921-003）
const modelsLoading = ref(false);
const threadsLoading = ref(false);
const threadsFailed = ref(false);
const gpuLoading = ref(false);
/** GPU 状态获取失败（区别于「尚未检测」，避免误当作「默认 CPU」） */
const gpuFailed = ref(false);
/** FEAT-052：模型下载状态（store 由 model-dl-progress 事件实时更新） */
const downloads = computed(() => contentStore.modelDownloads);

/* FEAT-061：下载源治理（镜像自检 + 自定义源） */
const probes = ref<ModelSourceProbe[]>([]);
const probeBusy = ref(false);
const probeErr = ref("");
const srcOpen = ref(false);
const customSrc = ref("");
const srcSaving = ref(false);
const probeTarget = ref("");
/** 是否已执行过「检测 GPU」 */
const detected = ref(false);

// ---- v6：CPU 线程数（适配不同硬件：可调 + 对比测速）----
const threadsInfo = ref<VcrThreadsInfo | null>(null);
const threadsSel = ref<number>(0);
const threadsBusy = ref(false);
const sweepBusy = ref(false);
/** 扫档结果（threads → 实测），用于展示与「采用」 */
const sweepResults = ref<VcrSweepEntry[]>([]);

const gpu = computed(() => contentStore.gpuStatus);
/** 检测到真正可用的本地 GPU 提供方（DirectML/CUDA 等，排除 Azure 云端） */
const gpuAvailable = computed(() => (gpu.value?.gpu.length ?? 0) > 0);
/** 当前是否已在 GPU 上推理 */
const accelerating = computed(() => gpu.value?.use_gpu === true);

// ---- FEAT-053：实测验证（登记值 vs 会话实测值对照 + 推理测速） ----
const benchBusy = ref(false);
const benchResult = ref<VcrBenchmarkResult | null>(null);
/** 最近一次 CPU / GPU 测速均值（不同 provider 各记一份，两者都有时显示提速比） */
const benchMs = ref<{ cpu: number | null; gpu: number | null }>({ cpu: null, gpu: null });

/** vision 塔会话实测事实（切换后台加载期间为 undefined）——与 current 对照确认切换真生效 */
const groundTruth = computed(() => modelsInfo.value?.loaded ?? null);
/** 当前生效档位的说明（切换后需重建索引的提醒） */
const activeModel = computed(() =>
  (modelsInfo.value?.models ?? []).find((m) => m.active) ?? null,
);
/** v5：切换档位后语义索引需重建（旧档向量维度/空间不同，不能混用） */
const clipSwitchTip = computed(() => {
  const m = activeModel.value;
  if (!m) return "";
  return `${m.dim} 维 · 输入 ${m.size}×${m.size}`;
});
const providerLabel = (p: string): string =>
  p.startsWith("Dml")
    ? "DirectML"
    : p.startsWith("Cuda")
      ? "CUDA"
      : p.replace("ExecutionProvider", "");
const groundTruthOnGpu = computed(() =>
  (groundTruth.value?.providers ?? []).some((p) => !p.startsWith("CPU")),
);
const groundTruthText = computed(() => {
  const f = groundTruth.value;
  if (!f) return "会话未加载（切换模型/开关加速后自动重建，可点测速触发）";
  const size = f.file_size ? ` · ${(f.file_size / 1e6).toFixed(0)}MB` : "";
  const prov = f.providers.length ? f.providers.map(providerLabel).join("+") : "?";
  const fallback = f.cpu_fallback ? " · ⚠ GPU 初始化失败已回退 CPU" : "";
  const th = f.threads ? ` · ${f.threads} 线程` : "";
  return `实际加载 ${f.file}${size} · 实际绑定 ${prov}${th}${fallback}`;
});
const benchText = computed(() => {
  const r = benchResult.value;
  if (!r) return "";
  const prov = r.providers.map(providerLabel).join("+");
  const { cpu, gpu: gpuMs } = benchMs.value;
  const speedup = cpu && gpuMs ? ` · 比 CPU 提速 ${(cpu / gpuMs).toFixed(1)}×` : "";
  return `平均 ${r.avg_ms}ms（最快 ${r.min_ms}ms · ${prov}）${speedup}`;
});

/** 测速：固定张量预热后计时（同时确保会话已按当前 provider 重建，顺手刷新实测展示） */
async function runBenchmark() {
  benchBusy.value = true;
  try {
    const r = await withTimeout(
      contentStore.benchmarkVcr(10, 2),
      PERF_TIMEOUT.benchmark,
      "benchmark_vcr",
    );
    benchResult.value = r;
    if (r.providers.some((p) => !p.startsWith("CPU"))) {
      benchMs.value.gpu = r.avg_ms;
    } else {
      benchMs.value.cpu = r.avg_ms;
    }
    void loadModels(); // 顺手刷新会话实测（不再单独发一次无人接管的请求）
  } catch (e) {
    notify.error("测速失败", String(e));
  } finally {
    benchBusy.value = false;
  }
}

/**
 * 三个数据源**各自独立**加载（BUG-2026-0921-003 回归修复）。
 *
 * 旧实现用 Promise.allSettled 等三者全部 settle 后才赋值 —— 只要「检测 GPU」慢
 * （历史上会等模型就绪 90s）或挂住，语义模型/线程数下拉即使早已拿到数据也一直是
 * 空 + disabled，用户看到的就是「不能选择模型和线程，一直转圈」。
 * 现在各自到位即渲染，并各自带超时与失败态（可单独重试）。
 */
async function loadModels(): Promise<boolean> {
  modelsLoading.value = true;
  modelsFailed.value = false;
  try {
    modelsInfo.value = await withTimeout(
      contentStore.fetchVcrModels(),
      PERF_TIMEOUT.read,
      "list_vcr_models",
    );
    selectedModel.value = modelsInfo.value.current ?? "";
    return true;
  } catch (e) {
    modelsFailed.value = true;
    console.warn("[perf-settings] list_vcr_models 失败:", e);
    return false;
  } finally {
    modelsLoading.value = false;
  }
}

async function loadThreads(): Promise<boolean> {
  threadsLoading.value = true;
  threadsFailed.value = false;
  try {
    const info = await withTimeout(
      contentStore.fetchVcrThreads(),
      PERF_TIMEOUT.read,
      "get_vcr_threads",
    );
    threadsInfo.value = info;
    if (!threadsSel.value) threadsSel.value = info.threads;
    return true;
  } catch (e) {
    threadsFailed.value = true;
    console.warn("[perf-settings] get_vcr_threads 失败:", e);
    return false;
  } finally {
    threadsLoading.value = false;
  }
}

/** 刷新 GPU 状态；silent=false 时把失败抛给调用方（「检测 GPU」按钮要提示） */
async function loadGpu(silent = true): Promise<boolean> {
  gpuLoading.value = true;
  gpuFailed.value = false;
  try {
    await withTimeout(contentStore.fetchGpuStatus(), PERF_TIMEOUT.read, "get_vcr_gpu_status");
    detected.value = true;
    return true;
  } catch (e) {
    gpuFailed.value = true;
    console.warn("[perf-settings] get_vcr_gpu_status 失败:", e);
    if (!silent) throw e;
    return false;
  } finally {
    gpuLoading.value = false;
  }
}

/** 并行拉取三者（互不阻塞）；非 silent 且 GPU/模型均失败 → 抛错（整体不可用） */
async function refreshAll(silent = false) {
  const [modelsOk, , gpuOk] = await Promise.all([loadModels(), loadThreads(), loadGpu(true)]);
  if (!modelsOk && !gpuOk) {
    if (!silent) throw new Error("识别服务不可用（模型清单与 GPU 状态均获取失败）");
    initFailed.value = true;
  }
}

// 行内重试：只重拉对应数据源，不牵连其他行
function retryModels() {
  void loadModels();
}
function retryThreads() {
  void loadThreads();
}
function retryGpu() {
  void loadGpu(true);
}

onMounted(async () => {
  try {
    // 三源并行、各自独立渲染；下载状态另有事件通道
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
        void loadModels();
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
/** FEAT-061：对首个未下载模型逐个源做 Range 探测（只取 1 字节，不下载整文件） */
async function runProbe() {
  const target = downloads.value.find((d) => !d.done) ?? downloads.value[0];
  if (!target) return;
  probeTarget.value = target.name;
  probeBusy.value = true;
  probeErr.value = "";
  try {
    probes.value = await contentStore.probeModelSources(target.name);
  } catch (e) {
    probes.value = [];
    probeErr.value = String(e);
  } finally {
    probeBusy.value = false;
  }
}

/** FEAT-061：展开/收起自定义源（展开时载入当前配置） */
async function toggleSources() {
  srcOpen.value = !srcOpen.value;
  if (!srcOpen.value) return;
  try {
    const info = await contentStore.getModelSources();
    customSrc.value = info.custom.join("\n");
  } catch (e) {
    notify.error("读取下载源配置失败", String(e));
  }
}

/** FEAT-061：保存自定义源并立即重测（自定义源优先于内置源） */
async function saveSources() {
  srcSaving.value = true;
  try {
    const list = customSrc.value
      .split(/\r?\n/)
      .map((s) => s.trim())
      .filter(Boolean);
    await contentStore.setModelSources(list);
    notify.success(
      "下载源已保存",
      list.length ? `自定义 ${list.length} 个源（下载时优先尝试）` : "已恢复为内置镜像",
    );
    await runProbe();
  } catch (e) {
    notify.error("保存下载源失败", String(e));
  } finally {
    srcSaving.value = false;
  }
}

function startDl(name: string) {
  contentStore.startModelDownload(name).catch((e) => notify.error("启动下载失败", String(e)));
}
function cancelDl(name: string) {
  contentStore.cancelModelDownload(name).catch((e) => notify.error("取消失败", String(e)));
}

/** v6：切换线程数（服务端持久化 + 后台重建会话；扫描/搜索立即受益） */
async function onThreadsChange() {
  threadsBusy.value = true;
  try {
    const info = await withTimeout(
      contentStore.setVcrThreads(threadsSel.value),
      PERF_TIMEOUT.write,
      "set_vcr_threads",
    );
    threadsInfo.value = info;
    threadsSel.value = info.threads;
    notify.success(
      `CPU 线程数已设为 ${info.threads}`,
      `默认按物理核推测为 ${info.default}（本机逻辑核 ${info.logical}）；` +
        `新设置对后续推理生效（会话已后台重建）`,
    );
  } catch (e) {
    notify.error("设置线程数失败", String(e));
    try {
      const info = await withTimeout(
        contentStore.fetchVcrThreads(),
        PERF_TIMEOUT.read,
        "get_vcr_threads",
      );
      threadsInfo.value = info;
      threadsSel.value = info.threads;
    } catch {
      /* 回显失败忽略 */
    }
  } finally {
    threadsBusy.value = false;
  }
}

/** v6：对比测速 —— 对语义图塔依次用各线程数实测（不改变当前设置） */
async function runSweep() {
  sweepBusy.value = true;
  try {
    const opts = threadsInfo.value?.options ?? [];
    const list = await withTimeout(
      contentStore.benchmarkVcrSweep("clip_vision", opts, 8, 2),
      PERF_TIMEOUT.sweep,
      "benchmark_vcr_sweep",
    );
    const ok = list.filter((r) => typeof r.avg_ms === "number");
    const base = ok.length ? Math.max(...ok.map((r) => r.avg_ms as number)) : 0;
    sweepResults.value = list
      .slice()
      .sort((a, b) => (a.avg_ms ?? 1e9) - (b.avg_ms ?? 1e9))
      .map((r) => ({ ...r, speedup: r.avg_ms && base ? +(base / r.avg_ms).toFixed(2) : undefined }));
    const best = sweepResults.value.find((r) => r.best);
    if (best?.avg_ms) {
      notify.info(
        `实测最快：${best.threads} 线程（${best.avg_ms} ms/张）`,
        "测速有 ±10% 波动，可点「采用」后跑一次扫描看实际感受",
      );
    }
  } catch (e) {
    notify.error("对比测速失败", String(e));
  } finally {
    sweepBusy.value = false;
  }
}

/** 采用扫档最优线程数 */
async function applyThreads(n: number) {
  threadsSel.value = n;
  await onThreadsChange();
}

/** 检测 GPU：只刷新 GPU 状态（模型/线程清单不随检测变化），失败明确提示 */
async function detect() {
  detectBusy.value = true;
  try {
    await loadGpu(false);
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
    const s = await withTimeout(
      contentStore.setVcrGpu(target),
      PERF_TIMEOUT.write,
      "set_vcr_gpu",
    );
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
    const info = await withTimeout(
      contentStore.setVcrModel(name),
      PERF_TIMEOUT.write,
      "set_vcr_model",
    );
    modelsInfo.value = info;
    selectedModel.value = info.current ?? name;
    notify.success(
      "语义模型已切换",
      info.clip_ready === false
        ? `${name} · 后台加载中，就绪后自动生效`
        : `${name} · ⚠ 换档后必须**重建语义索引**：到「扫描中心」对相册执行一次含「语义向量」的扫描` +
          `（旧档位向量不会被混用，未重建前语义搜索/分类会提示"尚未建立索引"）`,
    );
  } catch (e) {
    notify.error("切换模型失败", String(e));
    try {
      await loadModels();
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
      <!-- 1. 先选语义模型档位（适配不同硬件：B/16 轻量 → L/14-336 更强） -->
      <div class="mgps-row">
        <span class="mgps-label">语义模型</span>
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
            {{ m.label }}（{{ m.dim }} 维{{ m.bytes ? " · " + (m.bytes / 1e6).toFixed(0) + "MB" : "" }}）{{ m.downloaded ? "" : " —— 未下载" }}{{ m.active ? " ✓当前" : "" }}
          </option>
        </select>
        <span v-if="modelBusy" class="mgps-busy">切换中…</span>
        <span v-else-if="modelsLoading && !modelsInfo" class="mgps-busy">加载中…</span>
        <span v-else-if="modelsFailed" class="mgps-status err">
          模型清单加载失败（识别服务可能正在启动/升级）
          <button class="mgps-btn mgps-btn-sm" @click="retryModels">重试</button>
        </span>
        <span v-else-if="modelsInfo?.clip_ready === false" class="mgps-busy">模型加载中…</span>
        <span v-else-if="clipSwitchTip" class="mgps-status">{{ clipSwitchTip }}</span>
      </div>
      <p v-if="activeModel?.note" class="mgps-hint">{{ activeModel.note }}</p>

      <!-- 2. 检测 GPU -->
      <div class="mgps-row">
        <span class="mgps-label">运行硬件</span>
        <button class="mgps-btn" :disabled="detectBusy || gpuLoading" @click="detect">
          {{ detectBusy || gpuLoading ? "检测中…" : "🔍 检测 GPU" }}
        </button>
        <template v-if="gpuFailed && !gpu">
          <span class="mgps-status err">
            GPU 状态获取失败
            <button class="mgps-btn mgps-btn-sm" @click="retryGpu">重试</button>
          </span>
        </template>
        <template v-else-if="detected && gpu">
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

      <!-- 4. FEAT-053：实测验证 —— 会话铁证 + 推理测速（登记值之外的真相） -->
      <div class="mgps-row">
        <span class="mgps-label">会话实测</span>
        <span
          class="mgps-status"
          :class="{ ok: groundTruthOnGpu && !groundTruth?.cpu_fallback, err: groundTruth?.cpu_fallback }"
        >
          {{ groundTruthText }}
        </span>
      </div>
      <!-- v6：CPU 线程数（不同机器最优值不同 → 可调 + 一键对比测速） -->
      <div class="mgps-row">
        <span class="mgps-label">CPU 线程数</span>
        <select
          v-model.number="threadsSel"
          class="mgps-select mgps-select-sm"
          :disabled="threadsBusy || !threadsInfo"
          @change="onThreadsChange"
        >
          <option v-for="n in threadsInfo?.options ?? []" :key="n" :value="n">
            {{ n }} 线程{{ n === threadsInfo?.default ? "（推荐默认）" : "" }}
          </option>
        </select>
        <button class="mgps-btn" :disabled="sweepBusy" @click="runSweep">
          {{ sweepBusy ? "测速中…" : "📊 对比测速" }}
        </button>
        <span v-if="threadsBusy" class="mgps-busy">切换中…</span>
        <span v-else-if="threadsLoading && !threadsInfo" class="mgps-busy">加载中…</span>
        <span v-else-if="threadsFailed" class="mgps-status err">
          线程数加载失败
          <button class="mgps-btn mgps-btn-sm" @click="retryThreads">重试</button>
        </span>
        <span v-else-if="threadsInfo" class="mgps-status">
          生效 {{ threadsInfo.threads }} 线程 · 物理核约 {{ threadsInfo.physical_guess }}（逻辑 {{ threadsInfo.logical }}）
        </span>
      </div>
      <div v-if="sweepResults.length" class="mgps-sweep">
        <div
          v-for="r in sweepResults"
          :key="r.threads"
          class="mgps-sweep-row"
          :class="{ best: r.best, cur: r.threads === threadsInfo?.threads }"
        >
          <span class="mgps-sweep-th">{{ r.threads }} 线程<template v-if="r.threads === threadsInfo?.threads"> · 当前</template></span>
          <span class="mgps-sweep-ms">{{ r.avg_ms != null ? `${r.avg_ms} ms/张` : r.error ?? "失败" }}</span>
          <span class="mgps-sweep-sp">{{ r.speedup ? `相对最慢 ${r.speedup}×` : "" }}</span>
          <button v-if="r.best && r.threads !== threadsInfo?.threads" class="mgps-btn mgps-btn-sm" @click="applyThreads(r.threads)">
            采用最快
          </button>
        </div>
      </div>

      <div class="mgps-row">
        <span class="mgps-label">推理测速</span>
        <button class="mgps-btn" :disabled="benchBusy" @click="runBenchmark">
          {{ benchBusy ? "测速中…" : "📊 测速（人物检测通道；开/关加速各测一次可对比）" }}
        </button>
        <span v-if="benchResult" class="mgps-status" :class="{ ok: benchResult.providers.some((p) => !p.startsWith('CPU')) }">
          {{ benchText }}
        </span>
      </div>

      <p class="mgps-hint">
        <b>CPU 线程数</b>决定推理速度，且不同机器最优值不同：默认取「物理核数」（超线程的逻辑核收益低，
        实测本机 4→8 线程 CLIP 编码快约 1.6×）。点「📊 对比测速」会用各档线程数实测语义图塔并标出最快档，
        可一键「采用」；测速有 ±10% 波动，建议采用后再跑一次扫描感受实际耗时。
        （线程数不需要重建语义索引，改完下次扫描/搜索立即生效。）<br />
        默认使用 CPU 推理；检测到 GPU 后可开启加速（需 GPU 版运行时，如
        <code>onnxruntime-directml</code>）。GPU 加速对<b>人物检测 / 人脸 / OCR</b> 生效；
        语义模型默认固定 CPU —— AMD DirectML 对 fp16 图存在算子级数值
        bug（实测输出错误），故 fp16 档不做 GPU 加速；如需用核显加速语义索引，
        可下载 <b>B/16 fp32</b> 档（719MB，DirectML 实测 37.9ms/张，约为 fp16 CPU 的 2.4 倍快），
        该档的 GPU 开关需先做数值一致性验证后再启用。<br />
        语义模型档位切换后<b>必须重建语义索引</b>（不同档位维度/空间不同，旧向量不会被混用）：
        到「内容分类」页点「🔄 重建分类」，或对相册重新执行一次含「语义向量」的扫描。
        语义模型可在此直接下载（<code>chinese-clip</code> / <code>chinese-clip-fp32</code>）。
      </p>

      <!-- FEAT-052：模型下载（后台 + 进度 + 官方/镜像择优） -->
      <div class="mgps-row mgps-dl-head">
        <span class="mgps-label">📥 模型下载</span>
        <span class="mgps-hint">多镜像候选（自定义优先）逐个尝试，失败自动换源、支持断点续传；.pt 下载后自动导出 onnx</span>
      </div>

      <!-- FEAT-061：下载源治理（URL 不可达时不再静默挂起，可自检/自定义） -->
      <div class="mgps-src">
        <button class="mgps-btn mgps-btn-sm" :disabled="probeBusy" @click="runProbe">
          {{ probeBusy ? "检测中…" : "🔍 镜像源自检" }}
        </button>
        <button class="mgps-btn mgps-btn-sm" @click="toggleSources">
          {{ srcOpen ? "收起自定义源" : "自定义源" }}
        </button>
        <span v-if="probes.length" class="mgps-src-sum">
          {{ probeTarget }} · 可用 {{ probes.filter((p) => p.ok).length }} / {{ probes.length }}
        </span>
      </div>
      <p v-if="probeErr" class="mgps-status err">{{ probeErr }}</p>
      <div v-if="probes.length" class="mgps-src-list">
        <div v-for="p in probes" :key="p.url" class="mgps-src-row">
          <span class="mgps-src-host" :title="p.url">{{ p.host }}</span>
          <span class="mgps-src-tag" :class="p.ok ? 'ok' : 'bad'">
            {{ p.ok ? `可用 ${p.status}` : p.status ? `HTTP ${p.status}` : "不可用" }}
          </span>
          <span class="mgps-src-ms">{{ p.ms }} ms</span>
          <span class="mgps-src-note" :title="p.error ?? ''">
            {{ p.error ? p.error.slice(0, 70) : p.builtin ? "内置源" : "自定义源" }}
          </span>
        </div>
      </div>
      <div v-if="srcOpen" class="mgps-src-edit">
        <p class="mgps-hint">
          每行一个模板，必须含 <code>{repo}</code> 与 <code>{path}</code>；自定义源优先于内置源，下载按顺序尝试。
        </p>
        <textarea
          v-model="customSrc"
          class="mgps-src-text"
          spellcheck="false"
          placeholder="https://your-mirror.example.com/{repo}/resolve/main/{path}"
        ></textarea>
        <div class="mgps-row">
          <button class="mgps-btn mgps-btn-sm" :disabled="srcSaving" @click="saveSources">
            {{ srcSaving ? "保存中…" : "保存并检测" }}
          </button>
          <button class="mgps-btn mgps-btn-sm" :disabled="srcSaving" @click="customSrc = ''">
            清空自定义
          </button>
        </div>
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
              失败：{{ d.error ?? "" }}
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
/* FEAT-062：原为「透明底 + rgba(127,127,127,.4) 描边」，在弹窗里几乎看不出是按钮；
   改为实底浅色（用主题变量，浅色/深色两套模式都清晰），禁用态用实底灰。*/
.mgps-btn {
  padding: 5px 14px;
  border-radius: 8px;
  border: 1px solid rgba(57, 108, 216, 0.45);
  background: var(--color-primary-soft);
  color: var(--color-text);
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
  transition: border-color 0.15s, background 0.15s;
}
.mgps-btn:hover:not(:disabled) {
  border-color: #396cd8;
  background: rgba(57, 108, 216, 0.28);
}
.mgps-btn:disabled {
  background: rgba(127, 127, 127, 0.14);
  border-color: rgba(127, 127, 127, 0.28);
  color: var(--color-text-3);
  cursor: wait;
}

.mgps-accel {
  padding: 5px 14px;
  border-radius: 8px;
  border: 1px solid rgba(127, 127, 127, 0.45);
  background: rgba(127, 127, 127, 0.12);
  color: var(--color-text);
  font-size: 12.5px;
  cursor: not-allowed;
  transition: border-color 0.15s, background 0.15s;
}
.mgps-accel:disabled {
  background: rgba(127, 127, 127, 0.07);
  border-color: rgba(127, 127, 127, 0.2);
  color: var(--color-text-3);
}
.mgps-accel:not(:disabled) {
  cursor: pointer;
  border-color: rgba(106, 141, 240, 0.75);
}
.mgps-accel:not(:disabled):hover {
  background: rgba(57, 108, 216, 0.22);
  border-color: #396cd8;
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
  border: 1px solid rgba(127, 127, 127, 0.45);
  background: rgba(127, 127, 127, 0.1);
  color: var(--color-text);
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
.mgps-select-sm {
  flex: 0 0 170px;
  min-width: 0;
}
.mgps-sweep {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px 10px;
  border-radius: 10px;
  background: rgba(127, 127, 127, 0.1);
}
.mgps-sweep-row {
  display: flex;
  align-items: center;
  gap: 12px;
  font-size: 12.5px;
}
.mgps-sweep-row.best .mgps-sweep-th {
  font-weight: 700;
  color: #15803d;
}
.mgps-sweep-row.cur .mgps-sweep-th {
  text-decoration: underline;
}
.mgps-sweep-th { min-width: 110px; }
.mgps-sweep-ms { min-width: 110px; font-family: ui-monospace, monospace; }
.mgps-sweep-sp { opacity: 0.7; }
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

/* FEAT-061：下载源自检 / 自定义源 */
.mgps-src {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.mgps-src-sum {
  font-size: 12px;
  opacity: 0.75;
}
.mgps-src-list {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 8px 10px;
  border-radius: 10px;
  background: rgba(127, 127, 127, 0.1);
}
.mgps-src-row {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 12px;
}
.mgps-src-host {
  min-width: 150px;
  font-family: ui-monospace, monospace;
}
.mgps-src-tag {
  padding: 0 6px;
  border-radius: 999px;
  font-size: 11px;
}
.mgps-src-tag.ok {
  background: rgba(21, 128, 61, 0.18);
  color: #15803d;
}
.mgps-src-tag.bad {
  background: rgba(214, 69, 69, 0.16);
  color: #d64545;
}
.mgps-src-ms {
  min-width: 62px;
  font-family: ui-monospace, monospace;
  opacity: 0.8;
}
.mgps-src-note {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  opacity: 0.7;
}
.mgps-src-edit {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.mgps-src-text {
  width: 100%;
  min-height: 62px;
  resize: vertical;
  padding: 6px 8px;
  border-radius: 8px;
  border: 1px solid rgba(127, 127, 127, 0.45);
  background: rgba(127, 127, 127, 0.1);
  color: var(--color-text);
  font: inherit;
}
.mgps-src-text:focus {
  outline: none;
  border-color: #396cd8;
}
/* 下载失败原因不再截 40 字：整行可读、可换行 */
.mgps-status.err {
  color: #d64545;
  white-space: normal;
  word-break: break-all;
}
</style>
