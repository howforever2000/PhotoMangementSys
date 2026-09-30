<script setup lang="ts">
/**
 * 扫描性能设置（FEAT-064）—— 相册扫描分组工具的 ⚙ 性能设置弹窗内容
 *
 * 解决两件事：
 *   1. **多线程扫描的核数可调**：本工具是 EXIF 文件扫描（IO/CPU 混合），
 *      线程数直接决定耗时，且不同机器最优值差异很大（老双核 2 线程即满、
 *      16 核本 8~12 线程收益递减、机械盘上过多线程反而变慢）。
 *   2. **根据当前相册实测推荐最优线程**：「📊 实测推荐」会用**当前目录的真实文件**
 *      在若干档位下实测，按「达峰值 95% 的最小档」给出最优值 ——
 *      比纯拓扑推断更贴合这个相册（大图/小图、SSD/HDD 都会影响结论）。
 *
 * 与「识别性能设置」（ModelGpuSettings）的关系：
 *   那边是 ONNX 推理线程数（纯计算，影响识别速度）；这边是**文件扫描**线程数
 *   （IO 密集，影响扫描/解析地名/移动的出列速度）。两套设置独立生效、互不影响。
 */
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { ScanPerfTopology, ScanCalibration } from "../types/photo";
import { useNotify } from "../composables/useNotify";

const props = defineProps<{
  /** 当前相册目录（实测推荐要用它取样；为空则禁用实测按钮） */
  dir?: string;
  /** 是否递归子目录（实测取样口径需与扫描一致） */
  recursive?: boolean;
}>();

const notify = useNotify();

const topo = ref<ScanPerfTopology | null>(null);
const loading = ref(false);
const failed = ref(false);
const errMsg = ref("");

/** 选择中的线程数（0 = 跟随推荐） */
const selected = ref(0);
const saving = ref(false);

/** 实测校准 */
const calibrating = ref(false);
const calib = ref<ScanCalibration | null>(null);
const calibErr = ref("");

/** GPU 加速开关（仅提示用途：扫描是 CPU/IO 负载，GPU 不加速扫描阶段） */
const effectiveText = computed(() => {
  const t = topo.value;
  if (!t) return "";
  const tail = t.saved == null ? "（跟随推荐）" : "（自定义）";
  return `${t.effective} 线程${tail}`;
});

const diskLabel = computed(() => {
  const k = topo.value?.disk_kind ?? "";
  if (!k) return "未知";
  const l = k.toLowerCase();
  if (l.includes("ssd")) return "SSD 固态盘";
  if (l.includes("hdd")) return "HDD 机械盘";
  return k;
});

/** 本机画像：一句话让用户知道"我这台机器什么水平" */
const machineText = computed(() => {
  const t = topo.value;
  if (!t) return "";
  const hy = t.hybrid ? " · 大小核混合" : "";
  const est = t.estimated ? "（推断）" : "";
  return `${t.physical} 物理核${est} / ${t.logical} 逻辑核${hy} · ${diskLabel.value}`;
});

async function load() {
  loading.value = true;
  failed.value = false;
  try {
    const t = await invoke<ScanPerfTopology>("get_scan_perf");
    topo.value = t;
    // 未自定义 → 下拉显示推荐值（用户不改就是推荐值）
    selected.value = t.saved ?? t.recommended;
    errMsg.value = "";
  } catch (e) {
    failed.value = true;
    errMsg.value = String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(load);

/** 保存线程数（None 语义：等于推荐值就存"跟随推荐"，避免推荐值变了用户还锁在旧值） */
async function apply(n: number) {
  if (!topo.value) return;
  saving.value = true;
  try {
    const isFollow = n === topo.value.recommended;
    const t = await invoke<ScanPerfTopology>("set_scan_perf", {
      threads: isFollow ? null : n,
    });
    topo.value = t;
    selected.value = t.saved ?? t.recommended;
    notify.success(
      isFollow ? `已跟随推荐（${t.recommended} 线程）` : `扫描线程数已设为 ${t.effective}`,
      isFollow
        ? "本机拓扑变化（换盘/插拔显示器不影响）时会自动采用新的推荐值"
        : `新设置对下一次扫描立即生效（当前任务不受影响）`,
    );
  } catch (e) {
    notify.error("设置扫描线程数失败", String(e));
    // 回显失败就把下拉拉回真实值
    selected.value = topo.value.saved ?? topo.value.recommended;
  } finally {
    saving.value = false;
  }
}

function onSelect() {
  void apply(selected.value);
}

/** 实测推荐：用当前相册的真实文件在多个档位下实测，算出最优线程 */
async function calibrate() {
  if (!props.dir) {
    notify.warning("请先选择文件夹", "实测推荐需要读取当前目录的文件样本");
    return;
  }
  calibrating.value = true;
  calibErr.value = "";
  calib.value = null;
  try {
    calib.value = await invoke<ScanCalibration>("calibrate_scan_threads", {
      path: props.dir,
      recurse: props.recursive ?? false,
    });
    const c = calib.value;
    notify.info(
      `实测最优：${c.best_threads} 线程`,
      `峰值在 ${c.peak_threads} 线程（${c.peak_per_sec} 张/秒）；` +
        `${c.best_threads} 线程已达其 ${pctOf(c.best_per_sec, c.peak_per_sec)}%，且提速 ${c.speedup}×`,
      6000,
    );
  } catch (e) {
    calibErr.value = String(e);
    notify.error("实测失败", String(e));
  } finally {
    calibrating.value = false;
  }
}

function pctOf(a: number, b: number): number {
  if (!b) return 100;
  return Math.round((a / b) * 100);
}

/** 采用实测推荐值 */
async function adoptCalib() {
  if (!calib.value) return;
  await apply(calib.value.best_threads);
}

/** 采用实测峰值档（追求极限速度时用） */
async function adoptPeak() {
  if (!calib.value) return;
  await apply(calib.value.peak_threads);
}

/** 实测结果按线程数升序（后端已排序，这里再确保一次） */
const calibRows = computed(() =>
  [...(calib.value?.results ?? [])].sort((a, b) => a.threads - b.threads),
);
const calibMaxRate = computed(() =>
  Math.max(1, ...calibRows.value.map((r) => r.per_sec ?? 0)),
);
</script>

<template>
  <div class="sps-wrap">
    <div class="sps-head">⚙ 扫描性能设置</div>

    <p v-if="failed" class="sps-hint sps-err">
      读取性能画像失败：{{ errMsg }}
      <button class="sps-btn sps-btn-sm" @click="load">重试</button>
    </p>

    <!-- 本机画像 -->
    <div class="sps-row">
      <span class="sps-label">本机硬件</span>
      <span v-if="loading" class="sps-dim">读取中…</span>
      <span v-else-if="topo" class="sps-status">{{ machineText }}</span>
    </div>

    <!-- 扫描线程数 -->
    <div class="sps-row">
      <span class="sps-label">扫描线程数</span>
      <select
        v-model.number="selected"
        class="sps-select"
        :disabled="saving || !topo"
        @change="onSelect"
      >
        <option v-for="n in topo?.options ?? []" :key="n" :value="n">
          {{ n }} 线程{{ n === topo?.recommended ? "（推荐）" : "" }}
        </option>
      </select>
      <span v-if="saving" class="sps-dim">保存中…</span>
      <span v-else-if="topo" class="sps-status">
        生效 <b>{{ effectiveText }}</b>
        <button
          v-if="topo.saved != null"
          class="sps-btn sps-btn-sm"
          :disabled="saving"
          @click="apply(topo.recommended)"
        >
          恢复推荐
        </button>
      </span>
    </div>

    <!-- 推荐理由 -->
    <p v-if="topo" class="sps-hint">
      <b>推荐依据</b>：{{ topo.reason }}。<br />
      扫描是「读文件 + 解析 EXIF」的 IO/CPU 混合负载：物理核决定真实并行上限，
      超线程只贡献部分吞吐，故推荐值为 <code>物理核 + (逻辑核-物理核)×2/5</code>；
      机械盘（HDD）随机读寻道敏感，会被压到 4 线程以内。
    </p>

    <!-- 实测推荐 -->
    <div class="sps-row sps-calib-head">
      <span class="sps-label">实测推荐</span>
      <button class="sps-btn" :disabled="calibrating || !dir" @click="calibrate">
        {{ calibrating ? "实测中…" : "📊 按当前相册实测最优线程" }}
      </button>
      <span v-if="!dir" class="sps-dim">请先在上方选择文件夹</span>
      <span v-else-if="calibrating" class="sps-dim">
        正在用该目录的真实文件跑各档位（含预热，约需数秒）
      </span>
    </div>

    <p v-if="calibErr" class="sps-hint sps-err">{{ calibErr }}</p>

    <!-- 实测结果 -->
    <div v-if="calib" class="sps-calib">
      <div class="sps-calib-sum">
        <span class="kpi">
          实测最优 <b class="ok">{{ calib.best_threads }}</b> 线程
        </span>
        <span class="kpi">
          吞吐 <b>{{ calib.best_per_sec }}</b> 张/秒
        </span>
        <span class="kpi">
          比单线程快 <b class="ok">{{ calib.speedup }}×</b>
        </span>
        <span class="kpi">
          峰值档 <b>{{ calib.peak_threads }}</b> 线程（{{ calib.peak_per_sec }} 张/秒）
        </span>
      </div>
      <p class="sps-hint">{{ calib.reason }}</p>

      <!-- 各档位对比条 -->
      <div class="sps-rows">
        <div
          v-for="r in calibRows"
          :key="r.threads"
          class="sps-calib-row"
          :class="{
            best: r.threads === calib.best_threads,
            peak: r.threads === calib.peak_threads,
            cur: r.threads === topo?.effective,
          }"
        >
          <span class="sps-calib-th">
            {{ r.threads }} 线程
            <template v-if="r.threads === calib.best_threads">· 最优</template>
            <template v-else-if="r.threads === calib.peak_threads">· 峰值</template>
            <template v-if="r.threads === topo?.effective">· 当前</template>
          </span>
          <div class="sps-bar">
            <div
              class="sps-bar-fill"
              :style="{ width: ((r.per_sec ?? 0) / calibMaxRate) * 100 + '%' }"
            ></div>
          </div>
          <span class="sps-calib-rate">
            {{ r.error ? r.error : `${r.per_sec} 张/秒 · ${r.per_file_ms} ms/张` }}
          </span>
        </div>
      </div>

      <div class="sps-calib-actions">
        <button
          class="sps-btn sps-btn-primary"
          :disabled="saving || calib.best_threads === topo?.effective"
          @click="adoptCalib"
        >
          采用最优 {{ calib.best_threads }} 线程
        </button>
        <button
          v-if="calib.peak_threads !== calib.best_threads"
          class="sps-btn"
          :disabled="saving || calib.peak_threads === topo?.effective"
          @click="adoptPeak"
        >
          采用峰值 {{ calib.peak_threads }} 线程
        </button>
      </div>
    </div>

    <p class="sps-hint sps-note">
      实测会取该目录<b>按文件大小分层抽样</b>的样本（大/中/小各约 70 张）在 6~7 个档位下各跑
      2 轮并取最快值，因此结果贴合这个相册的真实负载（大图解码慢、小图重在打开开销）；
      「最优」取<b>达峰值吞吐 95% 的最小线程数</b>——峰值档往往只快几个百分点却多占一倍
      CPU，性价比低。<br />
      修改线程数<b>不影响正在执行的任务</b>，对下一次扫描生效；本设置仅作用于
      「相册扫描分组工具」的文件扫描，与「识别性能设置」里的推理线程数相互独立。
    </p>
  </div>
</template>

<style scoped>
.sps-wrap {
  display: flex;
  flex-direction: column;
  gap: 10px;
  min-width: 420px;
  max-width: 560px;
  font-size: 13px;
}
.sps-head {
  font-size: 14px;
  font-weight: 700;
}
.sps-row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.sps-label {
  min-width: 76px;
  color: var(--ts-muted, #888);
  font-size: 12.5px;
}
.sps-select {
  padding: 5px 10px;
  border-radius: 7px;
  border: 1px solid var(--ts-btn-border, #ddd);
  background: var(--ts-btn-bg, #fff);
  color: var(--ts-text, #2c3e50);
  font-size: 13px;
  min-width: 150px;
}
.sps-btn {
  padding: 6px 12px;
  border-radius: 7px;
  border: 1px solid var(--ts-btn-border, #ddd);
  background: var(--ts-btn-bg, #fff);
  color: var(--ts-text, #2c3e50);
  cursor: pointer;
  font-size: 12.5px;
}
.sps-btn:hover:not(:disabled) {
  border-color: var(--color-primary);
  color: var(--color-primary);
}
.sps-btn:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}
.sps-btn-sm {
  padding: 3px 9px;
  font-size: 12px;
}
.sps-btn-primary {
  background: var(--color-primary);
  border-color: var(--color-primary);
  color: #fff;
}
.sps-btn-primary:hover:not(:disabled) {
  background: var(--color-primary-hover);
  color: #fff;
}
.sps-status {
  font-size: 12.5px;
  color: var(--ts-text, #2c3e50);
}
.sps-status b {
  color: var(--color-primary);
}
.sps-dim {
  font-size: 12px;
  color: var(--ts-muted, #888);
}
.sps-hint {
  font-size: 12px;
  line-height: 1.65;
  color: var(--ts-muted, #888);
  margin: 0;
}
.sps-hint code {
  background: rgba(127, 127, 127, 0.15);
  padding: 1px 5px;
  border-radius: 4px;
  font-family: "Consolas", monospace;
  font-size: 11.5px;
}
.sps-err {
  color: var(--color-danger);
}
.sps-note {
  border-top: 1px dashed var(--ts-panel-border, rgba(127, 127, 127, 0.25));
  padding-top: 8px;
}
/* 实测区 */
.sps-calib-head {
  margin-top: 2px;
}
.sps-calib {
  display: flex;
  flex-direction: column;
  gap: 8px;
  border: 1px solid var(--ts-panel-border, rgba(127, 127, 127, 0.25));
  border-radius: 9px;
  padding: 10px 12px;
}
.sps-calib-sum {
  display: flex;
  flex-wrap: wrap;
  gap: 14px;
  font-size: 12.5px;
}
.sps-calib-sum .kpi {
  color: var(--ts-muted, #888);
}
.sps-calib-sum .kpi b {
  font-size: 14px;
  color: var(--ts-text, #2c3e50);
}
.sps-calib-sum .kpi b.ok {
  color: var(--color-ok-vivid);
}
.sps-rows {
  display: flex;
  flex-direction: column;
  gap: 5px;
}
.sps-calib-row {
  display: grid;
  grid-template-columns: 108px 1fr 168px;
  align-items: center;
  gap: 8px;
  font-size: 12px;
}
.sps-calib-th {
  color: var(--ts-muted, #888);
}
.sps-bar {
  height: 8px;
  border-radius: 4px;
  background: rgba(127, 127, 127, 0.18);
  overflow: hidden;
}
.sps-bar-fill {
  height: 100%;
  border-radius: 4px;
  background: linear-gradient(90deg, var(--color-primary), #5a8ce8);
  transition: width 0.3s ease;
}
.sps-calib-rate {
  color: var(--ts-muted, #888);
  font-family: "Consolas", monospace;
  font-size: 11.5px;
  text-align: right;
}
.sps-calib-row.best .sps-calib-th {
  color: var(--color-ok-vivid);
  font-weight: 600;
}
.sps-calib-row.best .sps-bar-fill {
  background: linear-gradient(90deg, var(--color-ok-vivid), #22c55e);
}
.sps-calib-row.peak .sps-calib-th {
  color: var(--color-warn-text);
}
.sps-calib-row.cur .sps-calib-th::after {
  content: " ←";
  color: var(--color-primary);
}
.sps-calib-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
</style>
