<script setup lang="ts">
/**
 * 语义模型（FEAT-137：从「档位选择」变成「一行状态」）
 *
 * 为什么不再给选择器：以前有两档（B/16 fp16 / B/16 fp32），实测结论是
 * **fp16 档双输** ——
 *   · CPU 上更慢：74.4 / 113.7 ms/张（fp16）vs 60.6 / 76.4（fp32），批 8 / 批 32
 *   · 永远拿不到 GPU：fp16 权重在 DirectML 上有算子级数值 bug（BUG-2026-0910-006）
 *   · 精度等价：同批 32 张图 cos 最小 0.99999887
 * 于是只留 fp32 一档。旧向量标签（...-b16-fp16 / ...-b16-fp32）由宿主启动时
 * **重打成当前 id**，用户不需要重建语义索引（见 Rust `db::embedding::restamp_*`）。
 *
 * 保留的东西：磁盘体积（双塔实算）、就绪状态、手动刷新。
 * 「换档必须重建索引」的确认弹窗也随之消失 —— 没有档可换了。
 */
import { computed, onMounted, ref } from "vue";
import { useContentStore } from "../../stores/content";
import { PERF_TIMEOUT, withTimeout } from "../../utils/withTimeout";

defineProps<{ locked?: boolean }>();

const store = useContentStore();
const info = computed(() => store.vcrModels);
const active = computed(() => info.value?.models?.find((m) => m.active) ?? info.value?.models?.[0]);
const loading = ref(false);
const failed = ref(false);

const volume = computed(() => {
  const b = active.value?.bytes_disk || active.value?.bytes || 0;
  return b ? `${(b / 1e6).toFixed(0)}MB` : "";
});

const state = computed(() => {
  if (!active.value) return { text: "未读取到模型信息", tone: "dim" as const };
  if (!active.value.downloaded) return { text: "未下载（到「📦 模型管理」下载）", tone: "dim" as const };
  if (info.value?.clip_ready) return { text: "已加载", tone: "ok" as const };
  return { text: "未加载（首次语义搜索时自动加载）", tone: "dim" as const };
});

async function load() {
  loading.value = true;
  failed.value = false;
  try {
    await withTimeout(store.fetchVcrModels(), PERF_TIMEOUT.read, "list_vcr_models");
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
}

onMounted(() => void load());
</script>

<template>
  <section class="sec">
    <div class="sec-head">
      <h3 class="sec-title">语义模型</h3>
      <span class="sec-sub">单一档位：B/16（fp32 权重）</span>
    </div>

    <div class="sec-row">
      <span class="lbl">模型</span>
      <span class="val">Chinese-CLIP B/16 · 512 维</span>
      <span v-if="volume" class="dim">双塔 {{ volume }}</span>
      <span class="stat" :class="state.tone">{{ state.text }}</span>
      <button v-if="failed" class="btn btn-sm" :disabled="loading" @click="load">
        重试
      </button>
      <button v-else class="btn btn-sm" :disabled="loading" title="重新读取状态" @click="load">
        ⟳
      </button>
    </div>

    <p class="hint">
      以前有两档（fp16 / fp32）可选，已下线：实测 <b>fp16 在 CPU 上反而慢 18~33%</b>
      （x86 上要反复做精度转换），而且它在 DirectML 上有数值 bug、<b>永远用不上 GPU</b>；
      两档精度等价（cos ≥ 0.9999988）。现统一用 fp32 权重，
      <b>设备跟随「加速分配」/「GPU 加速」</b>，本机默认 CPU。
    </p>
    <p v-if="active?.note" class="hint dim">{{ active.note }}</p>
  </section>
</template>

<style scoped>
.sec {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.sec-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
}
.sec-title {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  color: var(--color-text);
}
.sec-sub {
  font-size: 12px;
  color: var(--color-text-3);
}
.sec-row {
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
.val {
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text);
}
.dim {
  font-size: 12px;
  color: var(--color-text-3);
}
.stat {
  font-size: 12.5px;
}
.stat.dim {
  color: var(--color-text-3);
}
.stat.ok {
  color: var(--color-success, #3aa76d);
}
.hint {
  margin: 0;
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--color-text-2);
}
.hint.dim {
  color: var(--color-text-3);
}
.btn {
  min-height: 26px;
  padding: 3px 9px;
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
.btn-sm {
  min-height: 26px;
  padding: 3px 9px;
}
</style>
