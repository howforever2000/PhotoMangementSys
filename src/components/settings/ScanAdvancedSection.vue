<script setup lang="ts">
/**
 * ③ 高级选项 · 扫描批次
 *
 * 批次**不是硬件开关**（对人物/文档识别无吞吐收益，后端 `clamp(4,64)` 默认 8），
 * 所以按约定收进「高级选项」折叠区，不再出现在两个扫描面板的可见行里。
 * 值存在 Pinia 的 `scanBatch`（localStorage 持久化），相册扫描与全局扫描共用一份。
 *
 * P23：加「实测最优批次」—— 批次推荐此前**永远显示默认值**（`batch_calibration.json`
 * 从未被写过），用户没有任何办法让它变成「本机实测」。本组件用同一套方法学
 * （同一批缩略图 + 交错轮转 + 每档预热1/正式2轮取最小）把曲线测出来。
 */
import { computed, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { BATCH_OPTIONS, useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";

const store = useContentStore();
const notify = useNotify();

const batch = computed({
  get: () => store.scanBatch,
  set: (n: number) => store.setScanBatch(n),
});

/** 单档实测结果 */
interface BatchRow {
  batch: number;
  requests?: number;
  ms_per_photo?: number;
  total_ms?: number;
  encoded?: number;
  error?: string;
}

/** `calibrate_scan_batch` 返回结构（对应 vcr_settings.rs 的 report） */
interface BatchCalib {
  cached?: boolean;
  sample: number;
  total: number;
  levels: number[];
  results: BatchRow[];
  optimal_batch: number;
  peak_batch: number;
  peak_ms: number;
  default_batch: number;
  default_ms: number | null;
  /** 相对默认档的收益比（0~1） */
  gain: number;
  /** 差异不足 5% → 保持默认即可 */
  insignificant: boolean;
  /** 拟合 cost = fit_a + fit_b/n 的两个系数 */
  fit_a: number;
  fit_b: number;
  est_secs: number;
  reason: string;
}

const calibrating = ref(false);
const calibErr = ref("");
const calib = ref<BatchCalib | null>(null);
const prog = ref<{ done: number; total: number } | null>(null);

/** 进度：后端每测完一档 emit 一次（扫档可达 1 分钟，没有进度会像卡死） */
const unlisten = listen<{ kind?: string; done: number; total: number }>(
  "scan-calib-progress",
  (e) => {
    if (e.payload?.kind === "batch") {
      prog.value = { done: e.payload.done, total: e.payload.total };
    }
  },
).catch(() => null);

onUnmounted(() => {
  void unlisten.then((f) => f?.());
});

/** 实测：不传 path → 后端自动取缩略图缓存（真实扫描送进 /embed_batch 的就是缩略图） */
async function calibrate(force = false) {
  calibrating.value = true;
  calibErr.value = "";
  calib.value = null;
  prog.value = null;
  try {
    const r = await invoke<BatchCalib>("calibrate_scan_batch", {
      path: null,
      recurse: false,
      force,
    });
    calib.value = r;
    const c = r.cached ? "（复用 30 天内结论）" : "";
    if (r.insignificant) {
      notify.info(
        `实测：各档差异不足 5%，保持默认 ${r.default_batch} 即可${c}`,
        `${r.reason}`,
        7000,
      );
    } else {
      notify.success(
        `实测最优批次：${r.optimal_batch} 张/批${c}`,
        `比默认 ${r.default_batch} 快 ${(r.gain * 100).toFixed(0)}%（${r.default_ms} → ${r.peak_ms} ms/张）`,
        7000,
      );
    }
  } catch (e) {
    calibErr.value = String(e);
    notify.error("批次实测失败", String(e));
  } finally {
    calibrating.value = false;
    prog.value = null;
  }
}

/** 采用实测出的最优档 */
function adopt() {
  const n = calib.value?.optimal_batch;
  if (n) {
    batch.value = n;
    notify.success(`已采用 ${n} 张/批`, "对下一次扫描生效");
  }
}

const busyText = computed(() => {
  if (!calibrating.value) return "";
  if (prog.value) return `实测中 ${prog.value.done}/${prog.value.total} 档…`;
  return "实测中…";
});
</script>

<template>
  <div class="adv-row">
    <label class="lbl" for="scan-batch">扫描批次</label>
    <select id="scan-batch" v-model.number="batch" class="sel" :disabled="calibrating">
      <option v-for="b in BATCH_OPTIONS" :key="b" :value="b">{{ b }} 张/批</option>
    </select>
    <span class="stat">每批一个识别请求 · 改动对下一次扫描生效</span>

    <!-- P23：实测入口 —— 批次推荐此前永远是「默认值」，没有任何办法变成「本机实测」 -->
    <button
      class="btn btn-sm"
      type="button"
      :disabled="calibrating"
      :title="'用真实缩略图对 4/8/16/32/64 各档实测 · 约 1 分钟'"
      @click="calibrate(false)"
    >
      {{ busyText || "📊 实测最优批次" }}
    </button>
    <button
      v-if="calibrating"
      class="btn btn-sm"
      type="button"
      disabled
      aria-busy="true"
    >
      测速中…
    </button>
  </div>

  <!-- 实测结论：把「为什么是这个数」一次性讲清，并给一键采用 -->
  <div v-if="calib" class="calib">
    <p class="hint">
      <b>本机实测</b>（样本 {{ calib.sample }} / 共 {{ calib.total }} 张 ·
      拟合 cost ≈ {{ calib.fit_a }} + {{ calib.fit_b }}/n ms
      <span class="dim">单图成本 + 每请求固定开销</span>）
    </p>
    <table class="calib-tbl">
      <thead>
        <tr><th>批次</th><th>请求数</th><th>ms/张</th><th>相对默认</th></tr>
      </thead>
      <tbody>
        <tr
          v-for="r in [...calib.results].sort((a, b) => a.batch - b.batch)"
          :key="r.batch"
          :class="{ best: r.batch === calib.optimal_batch, cur: r.batch === batch }"
        >
          <td>{{ r.batch }}</td>
          <td>{{ r.requests ?? "—" }}</td>
          <td>{{ r.error ? `失败：${r.error}` : r.ms_per_photo }}</td>
          <td>
            <template v-if="!r.error && r.ms_per_photo != null && calib.default_ms">
              {{
                (
                  (calib.default_ms - r.ms_per_photo) /
                  calib.default_ms *
                  100
                ).toFixed(0)
              }}%
            </template>
            <template v-else>—</template>
          </td>
        </tr>
      </tbody>
    </table>
    <p class="hint">{{ calib.reason }}</p>
    <div class="calib-acts">
      <button
        v-if="calib.optimal_batch !== batch"
        class="btn btn-sm btn-primary"
        type="button"
        @click="adopt"
      >
        采用实测值 {{ calib.optimal_batch }} 张/批
      </button>
      <button class="btn btn-sm" type="button" :disabled="calibrating" @click="calibrate(true)">
        重新实测
      </button>
      <span v-if="calib.cached" class="dim">复用 30 天内结论</span>
    </div>
  </div>
  <p v-else-if="calibErr" class="hint err">实测失败：{{ calibErr }}</p>

  <p class="hint">
    批次只影响<b>语义向量（CLIP）通道</b>一次请求带几张图：人物/文档识别是逐张处理，确实不受影响。
    但语义向量每次请求都有一笔<b>固定开销</b>（HTTP 往返 + 服务端调用），所以批越大摊得越薄、
    <b>但有上限</b>——增益集中在默认档往上的前两档，再往上只剩零头。
    本机到底哪档最快，点上面的「实测最优批次」用真实缩略图跑一遍。
    一般不用改；仅在内存吃紧时才往下调。
  </p>
</template>

<style scoped>
.adv-row {
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
.sel {
  min-width: 150px;
  padding: 7px 10px;
  border-radius: var(--radius-sm, 8px);
  border: 1px solid var(--color-border);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-size: 13px;
}
.sel:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--color-primary) 55%, transparent);
  outline-offset: 1px;
}
.stat {
  font-size: 12.5px;
  color: var(--color-text-3);
}
.hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--color-text-3);
}
.hint + .hint {
  margin-top: 6px;
}
.dim {
  color: var(--color-text-2, #9aa0a6);
}
.err {
  color: var(--color-danger, #d54941);
}

.calib {
  margin: 8px 0 4px;
  padding: 10px 12px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm, 8px);
  background: var(--color-surface-2);
}
.calib-tbl {
  width: auto;
  border-collapse: collapse;
  font-size: 12.5px;
  margin: 6px 0;
}
.calib-tbl th,
.calib-tbl td {
  padding: 3px 14px 3px 0;
  text-align: left;
  font-weight: 400;
  color: var(--color-text-2);
}
.calib-tbl th {
  color: var(--color-text-3);
  font-size: 12px;
}
/* 实测最优档 + 当前已选档 都高亮，方便一眼看出「是否需要改」 */
.calib-tbl tr.best td:first-child {
  color: var(--color-ok-vivid, var(--color-ok-text));
  font-weight: 600;
}
.calib-tbl tr.cur td:nth-child(2) {
  color: var(--color-primary, var(--color-primary));
}
.calib-acts {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  margin-top: 4px;
}
</style>
