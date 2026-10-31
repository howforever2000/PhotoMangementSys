<script setup lang="ts">
/**
 * 加速分配（FEAT-136）—— 一张「通道 → 设备」表，而不是一个开关
 *
 * 为什么必须是一个区而不是塞进「GPU 加速」：
 *   GPU（DirectML）与 NPU（OpenVINO）是**两个独立引擎**，能同时开工且不争队列；
 *   但各通道最优设备不同，一个全局开关表达不了：
 *
 *     det / ocr / face_rec → NPU（实测最优点；这三条是加锁次数最多的热通道）
 *     face_det（det_10g）  → GPU/CPU（NPU 上 116.5ms，比 DML 慢 7 倍，永久排除）
 *     clip_*               → CPU（fp16 档在 DML 上有数值 bug；且只占扫描 0.2%）
 *
 * 三条产品纪律：
 *   1. **组件是可选的**：没装就完全走 ONNX Runtime，行为与今天逐字一致。
 *   2. **只在 Intel 平台推荐**：OpenVINO 的 NPU 插件只认 Intel。
 *   3. **永远给退路**：「关闭分配」= 回到原来的全局 GPU 开关语义，不需要卸载组件。
 *
 * 数字口径（983 张真实相册，批 8 / P=6 / intra 8，AI 腿 ∥ 语义腿并发）：
 *   全 CPU 336.6 ms/张 → NPU 三条 + GPU 人脸检测 140.1 ms/张（−58.4%）。
 *   不承诺倍数：相册画像（人脸密度、OCR 触发率）不同，收益会变。
 */
import { computed, onMounted, ref } from "vue";
import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import { PERF_TIMEOUT, withTimeout } from "../../utils/withTimeout";

/** locked = 识别服务不可用时禁用所有写操作（与其它分区一致） */
const props = defineProps<{ locked?: boolean }>();
const locked = computed(() => props.locked === true);

const store = useContentStore();
const notify = useNotify();

const RUNTIME_MODEL = "openvino-runtime";
const CHANNELS: { key: string; label: string; note: string }[] = [
  { key: "det", label: "人物 / 车辆检测", note: "每张必跑" },
  { key: "face_det", label: "人脸检测", note: "按需（命中人物规则时）" },
  { key: "face_rec", label: "人脸编码", note: "每张有脸图 · 每脸一次" },
  { key: "ocr", label: "文字检测", note: "约 7 成图片" },
];

const busy = ref(false);
const loading = ref(false);
const failed = ref(false);

const accel = computed(() => store.accel);
const ov = computed(() => accel.value?.ov);
const npuOk = computed(() => accel.value?.available?.NPU === true);
const gpuOk = computed(() => accel.value?.available?.GPU === true);
const enabled = computed(() => accel.value?.enabled === true);
const installed = computed(() => ov.value?.installed === true);

const dl = computed(() => store.modelDownloads.find((d) => d.name === RUNTIME_MODEL) ?? null);
const downloading = computed(() => dl.value?.running === true);

/** 每通道「请求分配 → 实际生效」 */
const rows = computed(() =>
  CHANNELS.map((c) => {
    const want = accel.value?.channels?.[c.key] ?? "";
    const eff = accel.value?.effective?.[c.key] ?? "";
    return { ...c, want, eff };
  }),
);

const anyNpu = computed(() => rows.value.some((r) => r.eff === "NPU"));
const anyGpu = computed(() => rows.value.some((r) => r.eff === "GPU"));
const allCpu = computed(() => rows.value.every((r) => !r.eff || r.eff === "CPU"));

const deviceTone = (d: string) => (d === "NPU" ? "npu" : d === "GPU" ? "gpu" : "cpu");
const deviceLabel = (d: string) => (d === "NPU" ? "NPU" : d === "GPU" ? "DirectML" : d === "CPU" ? "CPU" : "未加载");

async function load(silent = false) {
  loading.value = true;
  failed.value = false;
  try {
    await withTimeout(store.fetchAccel(), PERF_TIMEOUT.read, "get_vcr_accel");
    if (!store.modelDownloads.length) void store.listModelDownloads().catch(() => {});
    return true;
  } catch (e) {
    failed.value = true;
    if (!silent) notify.error("读取加速分配失败", String(e));
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
    notify.success("开始下载", "约 15MB，装好后可把检测 / OCR / 人脸编码交给 NPU");
  } catch (e) {
    notify.error("下载失败", String(e));
  } finally {
    busy.value = false;
  }
}

/** 一键分配：用服务端推荐（能上 NPU 的给 NPU，剩下的给 GPU） */
async function enable() {
  busy.value = true;
  try {
    await withTimeout(store.setAccel(true), PERF_TIMEOUT.write, "set_vcr_accel");
    notify.success("已启用加速分配", "通道正在后台重建，几秒后本页会显示实际设备");
    // 后台重建是异步的：稍等再看一眼生效值（失败不打扰用户，下次进入页面会重读）
    setTimeout(() => void load(true), 4000);
  } catch (e) {
    notify.error("启用失败", String(e));
  } finally {
    busy.value = false;
  }
}

/** 关闭分配 = 回到原来的全局 GPU 开关语义（组件不用卸） */
async function disable() {
  busy.value = true;
  try {
    await withTimeout(store.setAccel(false), PERF_TIMEOUT.write, "set_vcr_accel");
    notify.success("已关闭加速分配", "所有通道回到原来的 GPU 开关 / CPU 语义");
    setTimeout(() => void load(true), 2500);
  } catch (e) {
    notify.error("关闭失败", String(e));
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="ac-head">
    <span class="lbl">加速分配</span>
    <span class="badge" :class="npuOk ? 'ok' : 'dim'">
      {{ npuOk ? "NPU 可用" : installed ? "无 NPU 设备" : "未装 NPU 组件" }}
    </span>
    <span class="badge" :class="gpuOk ? 'ok' : 'dim'">{{ gpuOk ? "DirectML 可用" : "无 DirectML" }}</span>
    <button v-if="failed" class="btn btn-sm" :disabled="locked || loading" @click="load(false)">重试</button>
  </div>

  <!-- 分配表：请求值 vs 实际生效，两列并排说清 -->
  <table v-if="accel" class="tbl">
    <thead>
      <tr>
        <th>通道</th>
        <th>触发</th>
        <th>分配</th>
        <th>实际</th>
      </tr>
    </thead>
    <tbody>
      <tr v-for="r in rows" :key="r.key">
        <td class="ch">{{ r.label }}</td>
        <td class="note">{{ r.note }}</td>
        <td>
          <span class="dev" :class="deviceTone(r.want)">{{ r.want ? deviceLabel(r.want) : "跟随开关" }}</span>
        </td>
        <td>
          <span class="dev" :class="deviceTone(r.eff)">{{ deviceLabel(r.eff) }}</span>
          <span v-if="accel.compiled?.[r.key] != null" class="comp">编译 {{ accel.compiled[r.key] }}s</span>
        </td>
      </tr>
    </tbody>
  </table>

  <!-- Intel 且未装组件：给下载入口 -->
  <template v-if="!installed">
    <p class="hint">
      装一个 <b>15MB</b>（装后 45MB）的运行时，就能把 <b>检测 / OCR / 人脸编码</b> 挪到 <b>NPU</b>，
      让核显（DirectML）只负责人脸检测 —— 两个引擎同时开工、互不排队。
      实测 983 张相册 <b>336.6 → 140.1 ms/张</b>。<b>不装也完全正常</b>，一切走 CPU。
    </p>
    <div class="row">
      <button class="btn btn-primary" :disabled="locked || busy || downloading" @click="download">
        {{ downloading ? "下载中…" : busy ? "启动中…" : "⬇ 下载 OpenVINO 运行时（约 15MB）" }}
      </button>
      <span v-if="dl?.error" class="err">{{ dl.error }}</span>
    </div>
    <div
      v-if="downloading && dl"
      class="bar"
      role="progressbar"
      :aria-valuenow="dl.bytes"
      :aria-valuemax="dl.total"
    >
      <i :style="{ width: `${Math.min(100, dl.total ? (dl.bytes / dl.total) * 100 : 0)}%` }"></i>
    </div>
  </template>

  <!-- 组件已装但 NPU 不可用（老 Intel / 驱动缺失）：不推荐开 -->
  <p v-else-if="!npuOk" class="hint">
    组件已装，但没探测到 <b>NPU</b> 设备{{ ov?.error ? `（${ov.error}）` : "" }}。
    这台机器继续用「GPU 加速」即可，不影响任何功能。
  </p>

  <!-- 可用：给启用/关闭 -->
  <template v-else>
    <p class="hint">
      <template v-if="enabled && anyNpu">
        已启用：<b>{{ anyNpu ? "NPU" : "" }}{{ anyNpu && anyGpu ? " + DirectML" : "" }}</b>
        分工执行{{ allCpu ? "（当前全部回落 CPU）" : "" }}。可在下方「实测与诊断」按通道看到实际提供方。
      </template>
      <template v-else-if="enabled">
        分配表已启用，但四条通道目前都落在 CPU —— 多半是组件刚装好、会话还没重建完，稍候刷新。
      </template>
      <template v-else>
        组件已就绪。启用后：检测 / OCR / 人脸编码 → <b>NPU</b>，人脸检测 → <b>DirectML</b>，
        语义索引仍按自己的档位规则（fp16 档固定 CPU）。
      </template>
    </p>
    <div class="row">
      <button v-if="!enabled" class="btn btn-primary" :disabled="locked || busy" @click="enable">
        {{ busy ? "启用中…" : "🚀 启用加速分配" }}
      </button>
      <button v-else class="btn" :disabled="locked || busy" @click="disable">
        {{ busy ? "关闭中…" : "关闭分配（回到 GPU 开关语义）" }}
      </button>
      <span class="ver">运行时 {{ ov?.source ? ov.source.split("\\").pop() : "NPU" }}</span>
    </div>
  </template>
</template>

<style scoped>
.ac-head {
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
.tbl {
  width: 100%;
  border-collapse: collapse;
  margin: 8px 0 10px;
  font-size: 12.5px;
}
.tbl th {
  text-align: left;
  font-weight: 600;
  color: var(--color-text-3);
  padding: 4px 8px 6px 0;
  border-bottom: 1px solid var(--color-border);
}
.tbl td {
  padding: 6px 8px 6px 0;
  border-bottom: 1px solid color-mix(in srgb, var(--color-border) 55%, transparent);
}
.ch {
  color: var(--color-text);
  font-weight: 600;
}
.note {
  color: var(--color-text-3);
}
.dev {
  display: inline-block;
  padding: 1px 8px;
  border-radius: 999px;
  font-size: 11.5px;
  font-weight: 600;
  border: 1px solid var(--color-border);
  color: var(--color-text-2);
}
.dev.npu {
  border-color: color-mix(in srgb, var(--color-primary) 55%, transparent);
  color: var(--color-primary-text);
  background: color-mix(in srgb, var(--color-primary) 12%, transparent);
}
.dev.gpu {
  border-color: color-mix(in srgb, var(--color-success, #3aa76d) 50%, transparent);
  color: var(--color-success, #3aa76d);
  background: color-mix(in srgb, var(--color-success, #3aa76d) 10%, transparent);
}
.comp {
  margin-left: 6px;
  font-size: 11px;
  color: var(--color-text-3);
}
.hint {
  margin: 6px 0 10px;
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--color-text-2);
}
.row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.badge {
  font-size: 11px;
  padding: 1px 7px;
  border-radius: 999px;
  border: 1px solid var(--color-border);
  color: var(--color-text-3);
}
.badge.ok {
  border-color: color-mix(in srgb, var(--color-success, #3aa76d) 55%, transparent);
  color: var(--color-success, #3aa76d);
}
.badge.dim {
  opacity: 0.75;
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
  opacity: 0.55;
  cursor: not-allowed;
}
.btn-primary {
  border-color: color-mix(in srgb, var(--color-primary) 60%, transparent);
  background: color-mix(in srgb, var(--color-primary) 14%, var(--color-surface-2));
}
.btn-sm {
  min-height: 26px;
  padding: 3px 9px;
  font-size: 12px;
}
.bar {
  height: 6px;
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
  overflow-wrap: anywhere;
}
.ver {
  font-size: 11px;
  color: var(--color-text-3);
}
</style>
