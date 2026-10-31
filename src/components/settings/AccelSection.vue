<script setup lang="ts">
/**
 * 加速 —— 全页**唯一**的加速开关，外加一张「哪条计算跑在哪个引擎」的对照表
 *
 * 为什么把旧的「GPU 加速」开关并进来：以前有两个开关（全局 GPU + 各通道分配），
 * 用户分不清谁管谁。现在只有一张分配表，按钮就是「开/关」，加速对象直接列在下
 * 面 —— 不再需要用户从文案里推断「这个按钮到底影响什么」。
 *
 * 三条产品纪律：
 *   1. 默认**开**（服务端 auto 模式按硬件给推荐分配）：模型不该默认跑 CPU，
 *      CPU 要留给缩略图 / EXIF / 地点解码。
 *   2. 组件（OpenVINO 运行时）是**可选**的：没装就走 DirectML，NPU 那三条腿
 *      自动落回核显；关掉开关则全部回 CPU，行为与老版本一致。
 *   3. 只在有引擎可用时给按钮；这台机器什么都没有就直说「只跑 CPU」。
 */
import { computed, onMounted, ref } from "vue";
import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import { PERF_TIMEOUT, withTimeout } from "../../utils/withTimeout";

const props = defineProps<{ locked?: boolean }>();
const locked = computed(() => props.locked === true);

const store = useContentStore();
const notify = useNotify();

const RUNTIME_MODEL = "openvino-runtime";

/** 加速对象（顺序 = 触发频率，从高到低） */
const ROWS = [
  { key: "det", label: "人物 / 车辆检测", when: "每张" },
  { key: "ocr", label: "文字检测", when: "约 7 成" },
  { key: "face_rec", label: "人脸编码", when: "有脸时" },
  { key: "face_det", label: "人脸检测", when: "人物规则命中" },
];

const busy = ref(false);
const loading = ref(false);
const failed = ref(false);

const accel = computed(() => store.accel);
const ov = computed(() => accel.value?.ov);
const mode = computed(() => accel.value?.mode ?? "auto");
const enabled = computed(() => accel.value?.enabled === true);
const npuOk = computed(() => accel.value?.available?.NPU === true);
const gpuOk = computed(() => accel.value?.available?.GPU === true);
const anyEngine = computed(() => npuOk.value || gpuOk.value);
const installed = computed(() => ov.value?.installed === true);

const dl = computed(() => store.modelDownloads.find((d) => d.name === RUNTIME_MODEL) ?? null);
const downloading = computed(() => dl.value?.running === true);

const rows = computed(() =>
  ROWS.map((r) => {
    const want = accel.value?.channels?.[r.key] ?? "";
    const eff = accel.value?.effective?.[r.key] ?? "";
    return { ...r, want, eff };
  }),
);

const npuCount = computed(() => rows.value.filter((r) => r.eff === "NPU").length);
const gpuCount = computed(() => rows.value.filter((r) => r.eff === "GPU").length);

const summary = computed(() => {
  if (!enabled.value) return "已关闭 · 全部 CPU";
  if (!anyEngine.value) return "这台机器没有可加速的显卡 / NPU";
  const parts = [npuCount.value ? `NPU ${npuCount.value} 条` : "",
                 gpuCount.value ? `核显 ${gpuCount.value} 条` : ""].filter(Boolean);
  return parts.length ? parts.join(" + ") : "重建中…";
});

const devLabel = (d: string) =>
  d === "NPU" ? "NPU" : d === "GPU" ? "核显" : d === "CPU" ? "CPU" : "—";

async function load(silent = false) {
  loading.value = true;
  failed.value = false;
  try {
    await withTimeout(store.fetchAccel(), PERF_TIMEOUT.read, "get_vcr_accel");
    if (!store.modelDownloads.length) void store.listModelDownloads().catch(() => {});
    return true;
  } catch (e) {
    failed.value = true;
    if (!silent) notify.error("读取加速状态失败", String(e));
    return false;
  } finally {
    loading.value = false;
  }
}

onMounted(() => void load(true));

async function setAccel(on: boolean) {
  busy.value = true;
  try {
    await withTimeout(store.setAccel(on), PERF_TIMEOUT.write, "set_vcr_accel");
    notify.success(on ? "已开启加速" : "已关闭加速",
                   on ? "通道正在后台重建，几秒后生效" : "全部计算回到 CPU");
    setTimeout(() => void load(true), 4000);
  } catch (e) {
    notify.error("切换失败", String(e));
  } finally {
    busy.value = false;
  }
}

async function download() {
  busy.value = true;
  try {
    await store.startModelDownload(RUNTIME_MODEL);
    notify.success("开始下载", "装好后会自动用上 NPU");
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
    <span class="pill" :class="enabled && anyEngine ? 'ok' : 'dim'">{{ summary }}</span>
    <span v-if="mode === 'auto' && anyEngine" class="pill soft">默认</span>

    <span class="spacer"></span>

    <button v-if="failed" class="btn btn-sm" :disabled="locked || loading" @click="load(false)">
      重试
    </button>
    <button
      v-else
      class="btn btn-sm"
      :disabled="locked || busy || loading || !anyEngine"
      @click="setAccel(!enabled)"
    >
      {{ busy ? "切换中…" : enabled ? "关闭加速" : "开启加速" }}
    </button>
  </div>

  <!-- 加速对象：一行一条，右侧给实际设备 -->
  <ul v-if="accel" class="map">
    <li v-for="r in rows" :key="r.key">
      <span class="m-name">{{ r.label }}</span>
      <span class="m-when">{{ r.when }}</span>
      <span class="m-dev" :class="(r.eff || r.want).toLowerCase()">
        {{ devLabel(r.eff || r.want) }}
      </span>
    </li>
    <li class="cpu-only">
      <span class="m-name">缩略图 · EXIF · 地点</span>
      <span class="m-when">始终</span>
      <span class="m-dev cpu">CPU</span>
    </li>
  </ul>

  <!-- NPU 可用但组件未装：一行提示 + 下载 -->
  <div v-if="npuOk && !installed" class="notice">
    <span>装 OpenVINO 运行时（15MB）可让检测 / 文字 / 人脸编码走 NPU</span>
    <button class="btn btn-sm" :disabled="locked || busy || downloading" @click="download">
      {{ downloading ? "下载中…" : "下载" }}
    </button>
    <span v-if="dl?.error" class="err">{{ dl.error }}</span>
  </div>
  <div v-if="downloading && dl" class="bar" role="progressbar"
       :aria-valuenow="dl.bytes" :aria-valuemax="dl.total">
    <i :style="{ width: `${Math.min(100, dl.total ? (dl.bytes / dl.total) * 100 : 0)}%` }"></i>
  </div>
</template>

<style scoped>
.acc-head {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.acc-title {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  color: var(--color-text);
}
.spacer {
  flex: 1;
}
.pill {
  font-size: 11.5px;
  padding: 1px 8px;
  border-radius: 999px;
  border: 1px solid var(--color-border);
  color: var(--color-text-3);
  white-space: nowrap;
}
.pill.ok {
  border-color: color-mix(in srgb, var(--color-success, #3aa76d) 55%, transparent);
  color: var(--color-success, #3aa76d);
}
.pill.soft {
  opacity: 0.7;
}
.map {
  list-style: none;
  margin: 8px 0 0;
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
.cpu-only .m-name,
.cpu-only .m-dev {
  color: var(--color-text-3);
}
.notice {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  margin-top: 10px;
  font-size: 12.5px;
  color: var(--color-text-2);
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
