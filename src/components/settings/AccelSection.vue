<script setup lang="ts">
/**
 * 加速 —— 两个独立勾选（NPU / GPU），勾谁就用谁
 *
 * 叫「GPU」而不是「核显」：DirectML 同样跑**独显**（NVIDIA / AMD），
 * 这一项管的是「用 DirectML 的那块 GPU」，不特指集成显卡。
 *
 * 交互规则（就是用户脑子里的那句话）：
 *   · 只勾 GPU → 四条计算全在 GPU 上
 *   · 只勾 NPU → 能在 NPU 上的三条走 NPU，人脸检测回 CPU（GPU 没勾就不占它）
 *   · 两个都勾 → 自动分工：NPU 拿触发最频繁的三条，GPU 拿人脸检测（两个引擎并行）
 *   · 都不勾 → 全部 CPU（老行为）
 *
 * 不可用的选项**不渲染**：没有 NPU 硬件 / 没装运行时 → 不出现 NPU 勾选项
 * （未装运行时只保留一行「装 15MB 运行时」的入口，那不是一个假勾选框）。
 *
 * 默认态 = 服务端 auto：按硬件推荐（两个都有就都勾上）。
 */
import { computed, onMounted, ref } from "vue";
import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import { PERF_TIMEOUT, withTimeout } from "../../utils/withTimeout";
import type { AccelDevice } from "../../types/content";

const props = defineProps<{ locked?: boolean }>();
const emit = defineEmits<{ failed: [string]; recovered: [] }>();
const locked = computed(() => props.locked === true);

const store = useContentStore();
const notify = useNotify();

const RUNTIME_MODEL = "openvino-runtime";
/** 能上 NPU 的通道（与后端 config.OV_SHAPES 同集：这三个是 OpenVINO 编译过的） */
const NPU_KEYS = ["det", "ocr", "face_rec"] as const;
const ALL_KEYS = ["det", "face_det", "face_rec", "ocr"] as const;

/** 对照表：顺序 = 触发频率从高到低 */
const ROWS = [
  { key: "det", label: "人物 / 车辆检测", when: "每张" },
  { key: "ocr", label: "文字检测", when: "约 7 成" },
  { key: "face_rec", label: "人脸编码", when: "有脸时" },
  { key: "face_det", label: "人脸检测", when: "人物规则命中" },
  { key: "clip", label: "语义索引（CLIP）", when: "新照片" },
];

const busy = ref(false);
const loading = ref(false);
const saving = ref(false);

const accel = computed(() => store.accel);
const ov = computed(() => accel.value?.ov);
const npuOk = computed(() => accel.value?.available?.NPU === true);
const gpuOk = computed(() => accel.value?.available?.GPU === true);
const installed = computed(() => ov.value?.installed === true);

/** 勾选态 = 当前分配里出现了该设备（clip 不参与这两项，它自己有行内开关） */
const npuChecked = computed(() =>
  !saving.value && NPU_KEYS.some((k) => accel.value?.channels?.[k] === "NPU"));
const gpuChecked = computed(
  () =>
    !saving.value &&
    (["det", "ocr", "face_rec"] as const).some(
      (k) => accel.value?.channels?.[k] === "GPU",
    ),
);

/** 语义索引（CLIP）自己的设备：默认跟随 GPU，可单独改回 CPU */
const clipOnGpu = computed(() => accel.value?.channels?.clip === "GPU");
const clipEff = computed(
  () => accel.value?.effective?.clip ?? accel.value?.channels?.clip ?? "");

const rows = computed(() =>
  ROWS.map((r) => ({
    ...r,
    want: accel.value?.channels?.[r.key] ?? "",
    eff: accel.value?.effective?.[r.key] ?? "",
  })),
);

const dl = computed(() => store.modelDownloads.find((d) => d.name === RUNTIME_MODEL) ?? null);
const downloading = computed(() => dl.value?.running === true);

const devLabel = (d: string) =>
  d === "NPU" ? "NPU" : d === "GPU" ? "GPU" : d === "CPU" ? "CPU" : "—";

async function load(silent = false) {
  loading.value = true;
  try {
    await withTimeout(store.fetchAccel(), PERF_TIMEOUT.read, "get_vcr_accel");
    emit("recovered");
    if (!store.modelDownloads.length) void store.listModelDownloads().catch(() => {});
    return true;
  } catch (e) {
    emit("failed", String(e));
    if (!silent) notify.error("读取加速状态失败", String(e));
    return false;
  } finally {
    loading.value = false;
  }
}

onMounted(() => void load(true));

/** 按勾选算通道分配。
 *  语义索引（CLIP）**默认跟随 GPU**：勾上 GPU 就一起走 GPU（用户 2026-11-03 定调），
 *  那一行右侧仍可单独改回 CPU（`clipOverride=false`，用于只做增量重扫的场景）。
 */
function channelsFor(
  useNpu: boolean,
  useGpu: boolean,
  clipOverride?: boolean,
): Record<string, AccelDevice> {
  const out: Record<string, AccelDevice> = {};
  if (useNpu && useGpu) {
    for (const k of NPU_KEYS) out[k] = "NPU";
    out.face_det = "GPU";
  } else if (useNpu) {
    for (const k of NPU_KEYS) out[k] = "NPU";
    // GPU 没勾：人脸检测明确回 CPU（不能留空，空会跟随自动探测又跑回 GPU）
    out.face_det = "CPU";
  } else if (useGpu) {
    for (const k of ALL_KEYS) out[k] = "GPU";
  }
  // 语义索引：跟随 GPU（可被行内开关显式改成 CPU）
  const clipGpu = clipOverride ?? useGpu;
  out.clip = clipGpu && gpuOk.value ? "GPU" : "CPU";
  return out;
}

async function apply(useNpu: boolean, useGpu: boolean, clipOverride?: boolean) {
  saving.value = true;
  busy.value = true;
  try {
    const ch = channelsFor(useNpu, useGpu, clipOverride);
    const on = Object.keys(ch).length > 0;
    await withTimeout(store.setAccel(on, on ? ch : undefined), PERF_TIMEOUT.write, "set_vcr_accel");
    notify.success(on ? "已更新加速分配" : "已关闭加速", on ? "通道正在后台重建，几秒后生效" : "全部计算回到 CPU");
    setTimeout(() => void load(true), 4000);
  } catch (e) {
    saving.value = false;
    notify.error("切换失败", String(e));
  } finally {
    busy.value = false;
    saving.value = false;
  }
}

const toggleNpu = () => apply(!npuChecked.value, gpuChecked.value);
const toggleGpu = () => apply(npuChecked.value, !gpuChecked.value);
/** 语义索引单独切：保持两个勾选不变 */
const toggleClip = () => apply(npuChecked.value, gpuChecked.value, !clipOnGpu.value);

async function download() {
  busy.value = true;
  try {
    await store.startModelDownload(RUNTIME_MODEL);
    notify.success("开始下载", "装好后 NPU 选项会自动出现");
  } catch (e) {
    notify.error("下载失败", String(e));
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="acc-head">
    <h3 class="acc-title">加速</h3>
    <button class="btn btn-sm" :disabled="loading" @click="load(false)">刷新</button>
  </div>

  <!-- 两个独立勾选：不可用的不渲染 -->
  <div class="opts">
    <label v-if="npuOk && installed" class="opt" :class="{ on: npuChecked }">
      <input type="checkbox" :checked="npuChecked" :disabled="locked || busy" @change="toggleNpu" />
      <span class="o-name">NPU</span>
      <span class="o-note">检测 · 文字 · 人脸编码</span>
    </label>

    <div v-else-if="npuOk && !installed" class="opt ghost">
      <span class="o-name">NPU</span>
      <span class="o-note">需先装运行时</span>
      <button class="btn btn-sm" :disabled="locked || busy || downloading" @click="download">
        {{ downloading ? "下载中…" : "下载 15MB" }}
      </button>
      <span v-if="dl?.error" class="err">{{ dl.error }}</span>
    </div>

    <label v-if="gpuOk" class="opt" :class="{ on: gpuChecked }">
      <input type="checkbox" :checked="gpuChecked" :disabled="locked || busy" @change="toggleGpu" />
      <span class="o-name">GPU</span>
      <span class="o-note">全部计算</span>
    </label>

    <span v-if="!npuOk && !gpuOk" class="none">这台机器没有可用的加速引擎，只跑 CPU</span>
  </div>

  <div v-if="downloading && dl" class="bar" role="progressbar"
       :aria-valuenow="dl.bytes" :aria-valuemax="dl.total">
    <i :style="{ width: `${Math.min(100, dl.total ? (dl.bytes / dl.total) * 100 : 0)}%` }"></i>
  </div>

  <!-- 对照表：勾选结果落到每条计算上（CLIP 那一行的设备可点，单独切） -->
  <ul v-if="accel" class="map">
    <li v-for="r in rows" :key="r.key">
      <span class="m-name">{{ r.label }}</span>
      <span class="m-when">{{ r.when }}</span>
      <button
        v-if="r.key === 'clip' && gpuOk"
        class="m-dev link"
        :disabled="locked || busy"
        :title="clipOnGpu ? '切回 CPU：语义腿变慢，但不与识别抢 GPU' : '改用 GPU：首次导入大量照片时更快，日常增量反而拖慢识别'",
        @click="toggleClip"
      >
        {{ devLabel(r.eff || r.want) }}
      </button>
      <span v-else class="m-dev" :class="(r.eff || r.want).toLowerCase()">
        {{ devLabel(r.eff || r.want) }}
      </span>
    </li>
    <li class="cpu-only">
      <span class="m-name">缩略图 · EXIF · 地点</span>
      <span class="m-when">始终</span>
      <span class="m-dev cpu">CPU</span>
    </li>
  </ul>
  <p v-if="gpuOk && clipEff" class="tip">
    语义索引默认跟随 GPU（{{ clipOnGpu ? "现在走 GPU" : "现在走 CPU" }}）：
    首次导入大量照片时更快；只做增量重扫时可点右侧设备改回 CPU（省下约 30% 识别时间）。
  </p>
</template>

<style scoped>
.acc-head {
  display: flex;
  align-items: center;
  gap: 8px;
}
.acc-title {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  color: var(--color-text);
}
.opts {
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
  margin-top: 8px;
}
.opt {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 5px 10px;
  border-radius: var(--radius-sm, 8px);
  border: 1px solid var(--color-border);
  background: var(--color-surface-2);
  font-size: 12.5px;
  cursor: pointer;
  user-select: none;
}
.opt.on {
  border-color: color-mix(in srgb, var(--color-primary) 55%, transparent);
  background: color-mix(in srgb, var(--color-primary) 10%, var(--color-surface-2));
}
.opt.ghost {
  cursor: default;
  color: var(--color-text-3);
}
.opt input {
  margin: 0;
}
.o-name {
  font-weight: 700;
  color: var(--color-text);
}
.o-note {
  color: var(--color-text-3);
  font-size: 11.5px;
}
.none {
  font-size: 12.5px;
  color: var(--color-text-3);
}
.map {
  list-style: none;
  margin: 10px 0 0;
  padding: 0;
}
.map li {
  display: grid;
  grid-template-columns: 1fr 76px 52px;
  align-items: center;
  gap: 8px;
  padding: 5px 0;
  border-bottom: 1px solid color-mix(in srgb, var(--color-border) 45%, transparent);
  font-size: 12.5px;
}
.map li:last-child {
  border-bottom: none;
}
.m-name {
  color: var(--color-text);
}
.m-when {
  color: var(--color-text-3);
  font-size: 11.5px;
  text-align: right;
}
.m-dev {
  text-align: right;
  font-size: 11.5px;
  font-weight: 600;
  color: var(--color-text-3);
}
.m-dev.npu {
  color: var(--color-primary-text);
}
.m-dev.gpu {
  color: var(--color-success, #3aa76d);
}
.m-dev.link {
  border: 1px solid var(--color-border);
  border-radius: 6px;
  background: transparent;
  padding: 0 6px;
  cursor: pointer;
  font-family: inherit;
}
.m-dev.link:hover:not(:disabled) {
  border-color: var(--color-primary);
  color: var(--color-primary-text);
}
.tip {
  margin: 8px 0 0;
  font-size: 11.5px;
  line-height: 1.6;
  color: var(--color-text-3);
}
.cpu-only .m-name,
.cpu-only .m-dev {
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
.btn:hover:not(:disabled) {
  border-color: var(--color-primary);
  color: var(--color-primary-text);
}
.btn:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}
.bar {
  height: 5px;
  border-radius: 999px;
  overflow: hidden;
  background: var(--color-surface-2);
  margin-top: 8px;
}
.bar i {
  display: block;
  height: 100%;
  background: var(--color-primary);
  transition: width var(--dur-2);
}
.err {
  font-size: 12px;
  color: var(--color-danger, #d9534f);
}
</style>
