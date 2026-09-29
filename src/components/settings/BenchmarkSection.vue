<script setup lang="ts">
/**
 * ③ 高级选项 · 实测与诊断
 *
 * 这一区**没有一个「设置」**，全是「跑一次才知道」的实测结果与只读事实：
 *   - 线程对比测速：各线程档实测语义图塔，给出最快档供采用
 *   - 推理测速：固定张量测一次人物检测通道（可与关加速对拍）
 *   - 会话实测：ORT 真实绑定的 provider / 文件（登记值之外的真相）
 * 长任务（测速/扫档）**不能真的取消**：invoke 无取消语义（见 withTimeout.ts 注释），
 * 后端一旦开跑就跑到底。所以这里只把等待时长讲清楚，不做假的取消按钮。
 */
import { computed, ref } from "vue";
import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import { PERF_TIMEOUT, withTimeout } from "../../utils/withTimeout";
import type { VcrBenchmarkResult, VcrSweepEntry } from "../../types/content";

defineProps<{ locked?: boolean }>();

const store = useContentStore();
const notify = useNotify();

const threads = computed(() => store.vcrThreads);
const loaded = computed(() => store.vcrModels?.loaded ?? null);
// P20：文本塔事实也报上 —— 会话是**双塔分别加载**的，只报图塔体积会让人以为
// 「档位 719MB 却只加载 345MB」，其实 345 + 390 = 719，只是口径没写清。
const loadedText = computed(() => store.vcrModels?.loaded_text ?? null);

const sweepBusy = ref(false);
const sweepResults = ref<VcrSweepEntry[]>([]);
const benchBusy = ref(false);
const benchResult = ref<VcrBenchmarkResult | null>(null);

function providerLabel(p: string): string {
  if (p.startsWith("Dml")) return "DirectML";
  if (p.startsWith("Cuda")) return "CUDA";
  return p.replace("ExecutionProvider", "");
}

/** P22：provider 口径文案。
 *
 * `get_providers()` 返回的是 ORT 的 provider **候选序列**，DirectML 档恒为
 * `[Dml, CPU]` —— 末尾的 CPU 是 ORT 默认兕底项，**不代表没用上 GPU**。
 * 旧文案直接 `join("+")` 得到「DirectML+CPU」，与档位的「GPU 可加速」
 * 以及说明里的「GPU 默认未开启」三处打架，用户读成「选了 GPU 结果没用 GPU」。
 * 只有 `cpu_fallback == true`（GPU 初始化抛错后重试成功）才是真回退。 */
function providerText(f: { providers: string[]; cpu_fallback?: boolean }): string {
  if (!f.providers.length) return "?";
  if (f.cpu_fallback) return "CPU（⚠ GPU 初始化失败已回退）";
  const gpu = f.providers.filter((p) => !p.startsWith("CPU"));
  const cpu = f.providers.filter((p) => p.startsWith("CPU"));
  if (gpu.length) {
    const g = gpu.map(providerLabel).join("+");
    return cpu.length ? `${g}（GPU 已生效，另有 ${cpu.length} 项 CPU 兕底）` : `${g}（GPU）`;
  }
  return "CPU";
}

/** 会话实测：ORT 真正绑到哪个 provider、加载的是哪个文件（P20 统一体积口径） */
const groundTruthText = computed(() => {
  const f = loaded.value;
  if (!f) return "会话未加载（切换模型/开关加速后自动重建，点「测速」会触发重建）";
  const mb = (n: number) => `${(n / 1e6).toFixed(0)}MB`;
  // 图塔 + 文本塔 = 双塔合计（与档位标签里写死的「双塔 719MB」同一口径）
  const t = loadedText.value;
  const tower =
    f.file_size && t?.file_size
      ? `${f.file} · 图塔 ${mb(f.file_size)} + 文本塔 ${mb(t.file_size)} = 双塔 ${mb(f.file_size + t.file_size)}`
      : `${f.file}${f.file_size ? `（单文件 ${mb(f.file_size)}）` : ""}`;
  const th = f.threads ? ` · ${f.threads} 线程` : "";
  return `实际加载 ${tower} · ${providerText(f)}${th}`;
});
const groundTruthOnGpu = computed(() =>
  (loaded.value?.providers ?? []).some((p) => !p.startsWith("CPU")),
);

const benchText = computed(() => {
  const r = benchResult.value;
  if (!r) return "";
  const prov = r.providers.map(providerLabel).join("+");
  const { cpu, gpu: gpuMs } = benchMs.value;
  const speedup = cpu && gpuMs ? ` · 比 CPU 快 ${(cpu / gpuMs).toFixed(1)}×` : "";
  return `平均 ${r.avg_ms}ms（最快 ${r.min_ms}ms · ${prov}）${speedup}`;
});
/** 最近一次 CPU / GPU 测速各留一份，开加速前测过 CPU 才算得出提速比 */
const benchMs = ref<{ cpu: number | null; gpu: number | null }>({ cpu: null, gpu: null });

/** 线程扫档：对语义图塔依次用各线程数实测（不改变当前设置） */
async function runSweep() {
  sweepBusy.value = true;
  try {
    const opts = threads.value?.options ?? [];
    const list = await withTimeout(
      store.benchmarkVcrSweep("clip_vision", opts, 8, 2),
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

/** 采用扫档给出的最快档（写入 store，① 本机硬件的下拉会同步） */
async function applyThreads(n: number) {
  try {
    const info = await withTimeout(
      store.setVcrThreads(n),
      PERF_TIMEOUT.write,
      "set_vcr_threads",
    );
    notify.success(`CPU 线程数已设为 ${info.threads}`, "对后续推理立即生效（会话已在后台重建）");
    // P21：后端 set_threads 已 _reload_all() 销毁旧会话并按新线程数惰性重建，
    // 「会话实测」的事实随之改变 —— 不重新拉就会继续展示**销毁前**那次会话的
    // 线程/provider（实测矛盾：这边显示「4 线程 · 当前」，下面却写「8 线程」）。
    // 与 runBenchmark() 的做法对齐（它 117 行就调了 fetchVcrModels）。
    void store.fetchVcrModels();
  } catch (e) {
    notify.error("设置线程数失败", String(e));
  }
}

/** 固定张量测速（同时会按当前 provider 重建会话 → 顺手刷新「会话实测」） */
async function runBenchmark() {
  benchBusy.value = true;
  try {
    const r = await withTimeout(
      store.benchmarkVcr(10, 2),
      PERF_TIMEOUT.benchmark,
      "benchmark_vcr",
    );
    benchResult.value = r;
    if (r.providers.some((p) => !p.startsWith("CPU"))) {
      benchMs.value = { ...benchMs.value, gpu: r.avg_ms };
    } else {
      benchMs.value = { ...benchMs.value, cpu: r.avg_ms };
    }
    void store.fetchVcrModels();
  } catch (e) {
    notify.error("测速失败", String(e));
  } finally {
    benchBusy.value = false;
  }
}
</script>

<template>
  <!-- 线程对比测速 -->
  <div class="bench-row">
    <span class="lbl">线程测速</span>
    <button class="btn" :disabled="locked || sweepBusy" @click="runSweep">
      {{ sweepBusy ? "测速中…（最长约 6 分钟）" : "📊 对比各线程档" }}
    </button>
    <span class="stat">对语义图塔逐档实测，不改当前设置</span>
  </div>

  <div v-if="sweepBusy" class="bench-note">
    正在按档位测量（含预热）。后端任务无法中途取消，跑完才出结果——请耐心等待，期间可以继续用软件。
  </div>

  <div v-if="sweepResults.length" class="sweep">
    <div
      v-for="r in sweepResults"
      :key="r.threads"
      class="sweep-row"
      :class="{ best: r.best, cur: r.threads === threads?.threads }"
    >
      <span class="sweep-th">
        {{ r.threads }} 线程<template v-if="r.threads === threads?.threads"> · 当前</template>
        <template v-else-if="r.best"> · 最快</template>
      </span>
      <span class="sweep-ms">{{ r.avg_ms != null ? `${r.avg_ms} ms/张` : r.error ?? "失败" }}</span>
      <button
        v-if="r.best && r.threads !== threads?.threads"
        class="btn btn-sm"
        @click="applyThreads(r.threads)"
      >
        采用
      </button>
    </div>
  </div>

  <!-- 推理测速 -->
  <div class="bench-row">
    <span class="lbl">推理测速</span>
    <button class="btn" :disabled="locked || benchBusy" @click="runBenchmark">
      {{ benchBusy ? "测速中…（约 10 秒）" : "📊 测速（人物检测通道）" }}
    </button>
    <span v-if="benchResult" class="stat">{{ benchText }}</span>
  </div>
  <p class="hint">开/关加速各测一次可直接对比；测速同时会按当前 provider 重建会话。</p>

  <!-- 会话实测（只读事实） -->
  <div class="bench-row">
    <span class="lbl">会话实测</span>
    <span
      class="stat"
      :class="{
        ok: groundTruthOnGpu && !loaded?.cpu_fallback,
        err: loaded?.cpu_fallback,
      }"
    >
      {{ groundTruthText }}
    </span>
  </div>
</template>

<style scoped>
.bench-row {
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
.btn {
  min-height: 30px;
  padding: 6px 12px;
  border-radius: var(--radius-sm, 8px);
  border: 1px solid var(--color-border);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
}
.btn:hover:not(:disabled) {
  border-color: var(--color-primary);
  color: var(--color-primary);
}
.btn:focus-visible {
  outline: 2px solid rgba(57, 108, 216, 0.55);
  outline-offset: 2px;
}
.btn:disabled {
  background: var(--color-neutral-soft);
  color: var(--color-text-3);
  cursor: not-allowed;
}
.btn-sm {
  min-height: 24px;
  padding: 2px 8px;
  font-size: 11.5px;
}
.stat {
  font-size: 12.5px;
  color: var(--color-text-2);
}
.stat.ok {
  color: var(--color-ok);
}
.stat.err {
  color: var(--color-danger);
}
.hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--color-text-3);
}
.bench-note {
  font-size: 12px;
  line-height: 1.7;
  padding: 8px 10px;
  border-radius: var(--radius-sm, 8px);
  background: var(--color-neutral-soft);
  color: var(--color-text-2);
}
.sweep {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.sweep-row {
  display: grid;
  grid-template-columns: 110px 110px auto;
  align-items: center;
  gap: 8px;
  font-size: 12px;
}
.sweep-th {
  color: var(--color-text-2);
}
.sweep-ms {
  color: var(--color-text-3);
  font-family: Consolas, monospace;
  font-size: 11.5px;
}
.sweep-row.best .sweep-th {
  color: var(--color-ok);
  font-weight: 600;
}
.sweep-row.cur .sweep-th::after {
  content: " ←";
  color: var(--color-primary);
}
</style>
