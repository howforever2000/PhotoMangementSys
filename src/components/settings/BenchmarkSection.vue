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
import type { VcrBenchmarkResult } from "../../types/content";

defineProps<{ locked?: boolean }>();

const store = useContentStore();
const notify = useNotify();

const loaded = computed(() => store.vcrModels?.loaded ?? null);
// P20：文本塔事实也报上 —— 会话是**双塔分别加载**的，只报图塔体积会让人以为
// 「档位 719MB 却只加载 345MB」，其实 345 + 390 = 719，只是口径没写清。
const loadedText = computed(() => store.vcrModels?.loaded_text ?? null);

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
  color: var(--color-primary-text);
}
.btn:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--color-primary) 55%, transparent);
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
</style>
