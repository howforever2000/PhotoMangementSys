<script setup lang="ts">
/**
 * 人脸模型（档位）—— 性能设置里的一节
 *
 * 两档可选、**选择持久化**（服务端写 models/current_face.json，重启后仍生效）：
 *   precise 高精度 · det_10g + w600k_r50 —— 非人脸误检最少、聚类最稳（默认）
 *   light   轻量  · det_500m + w600k_mbf —— 快 6~7 倍，误杀/错挂明显更多
 *
 * ⚠ 换档 = 换人脸向量空间：服务端会拦住不匹配的扫描（persons.db 的 meta.emb_model），
 *   必须先去「人物」页点「重建人物库」再重新扫描 —— 二次确认里写明后果。
 *
 * 与 ClipTierSection 的分工：那边是「服务是否可用」的探针；本区**不重复探活**，
 * 由 locked 控制（服务不可用时不请求，避免两处同时报错）。
 */
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import { PERF_TIMEOUT, withTimeout } from "../../utils/withTimeout";
import ConfirmDialog from "../ConfirmDialog.vue";

const props = defineProps<{
  /** 服务不可用 / 父级锁定 → 本区锁定（不请求） */
  locked?: boolean;
  /** 父级「重试」时自增，触发重新加载 */
  retryToken?: number;
}>();

const store = useContentStore();
const notify = useNotify();

const loading = ref(false);
const errMsg = ref("");
const busy = ref(false);
const selected = ref("");
/** 换档需二次确认：会作废人物库向量（破坏性操作） */
const confirmOpen = ref(false);

const info = computed(() => store.faceTiers);
const active = computed(() => info.value?.models?.find((m) => m.active) ?? null);
const tip = computed(() => {
  const m = active.value;
  if (!m) return "";
  const mb = (n: number) => `${(n / 1e6).toFixed(1)}MB`;
  return `${m.det} + ${m.rec} · 合计 ${mb(m.bytes)}`;
});

async function load() {
  if (props.locked) return false;
  loading.value = true;
  errMsg.value = "";
  try {
    const v = await withTimeout(store.fetchFaceTiers(), PERF_TIMEOUT.read, "get_face_tier_info");
    selected.value = v.current ?? "";
    return true;
  } catch (e) {
    // 服务不可用已由 ClipTierSection 报横幅，这里只置自身错误态，不重复上报
    errMsg.value = String(e);
    return false;
  } finally {
    loading.value = false;
  }
}

onMounted(() => void load());
watch(
  () => props.retryToken,
  () => void load(),
);
watch(
  () => props.locked,
  (v) => {
    if (!v) void load();
  },
);

// 切换后服务端后台加载新档（r50 冷加载数秒）：face_ready=false 期间轮询，就绪即停
const readyTimer = ref<number | null>(null);
function stopReadyPoll() {
  if (readyTimer.value != null) {
    window.clearInterval(readyTimer.value);
    readyTimer.value = null;
  }
}
watch(
  () => info.value?.face_ready,
  (v) => {
    if (v !== false) {
      stopReadyPoll();
      return;
    }
    if (readyTimer.value == null) {
      readyTimer.value = window.setInterval(() => void load(), 1500);
    }
  },
  { immediate: true },
);
onUnmounted(stopReadyPoll);

function onChange() {
  confirmOpen.value = true;
}
function onCancel() {
  confirmOpen.value = false;
  // 回显真实值：服务端没改成功就绝不显示成已切换
  selected.value = info.value?.current ?? selected.value;
}
async function onConfirm() {
  confirmOpen.value = false;
  const name = selected.value;
  busy.value = true;
  try {
    const v = await withTimeout(store.setFaceTier(name), PERF_TIMEOUT.write, "set_face_tier");
    selected.value = v.current ?? name;
    notify.success(
      "人脸模型已切换",
      v.face_ready === false
        ? `${name} · 后台加载中，就绪后生效（换档后需先「重建人物库」再扫描）`
        : `${name} · ⚠ 需先在「人物」页点「重建人物库」，再重新扫描相册（新旧人脸向量不在同一空间，未重建前扫描会被拦下并提示）`,
    );
  } catch (e) {
    notify.error("切换人脸模型失败", String(e));
    void load();
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <section class="sec">
    <div class="sec-head">
      <h3 class="sec-title">人脸模型</h3>
      <span class="sec-sub">精度优先还是速度优先；选择会被记住（重启仍生效）</span>
    </div>

    <div class="sec-row">
      <label class="lbl" for="face-tier">模型档位</label>
      <select
        id="face-tier"
        v-model="selected"
        class="sel"
        :disabled="locked || busy || loading || !info"
        @change="onChange"
      >
        <option
          v-for="m in info?.models ?? []"
          :key="m.name"
          :value="m.name"
          :disabled="!m.downloaded"
        >
          {{ m.label }} · 合计 {{ (m.bytes / 1e6).toFixed(1) }}MB{{
            m.downloaded ? "" : "（未下载）"
          }}{{ m.active ? " ✓当前" : "" }}
        </option>
      </select>

      <span v-if="busy || loading" class="stat dim">切换中…</span>
      <span v-else-if="errMsg" class="stat err" :title="errMsg">
        清单加载失败：{{ errMsg.slice(0, 60) }}{{ errMsg.length > 60 ? "…" : "" }}
        <button class="btn btn-sm" @click="load()">重试</button>
      </span>
      <span v-else-if="info?.face_ready === false" class="stat dim">模型加载中…</span>
      <span v-else-if="tip" class="stat">{{ tip }}</span>
    </div>

    <p v-if="active" class="hint">
      <b>当前档：{{ active.label }}</b> · {{ active.speed }} · {{ active.accuracy }}
    </p>
    <p class="hint">
      轻量档（det_500m + w600k_mbf）检测/识别约快 <b>6~7 倍</b>，代价是
      <b>非人脸误检与聚类错挂明显更多</b>（旧库实测：1255/9866 张脸与自身质心相似度 &lt;0.60）。
      只在机器跑不动、或想先快速粗分一遍时选它；追求准确度请用高精度档。
    </p>

    <ConfirmDialog
      :visible="confirmOpen"
      title="切换人脸模型档位？"
      message="换档会更换人脸向量空间：新旧向量不可比，人物库不会混用。切换后必须在「智慧相册 → 人物」页点「重建人物库」（自动备份 persons.db），再对相册重新扫描，否则扫描会被拦下并提示。现有人的改名与合并结果会随重建重置。"
      confirm-text="确认切换"
      cancel-text="取消"
      :danger="true"
      @confirm="onConfirm"
      @cancel="onCancel"
    />
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
.sel {
  min-width: 240px;
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
.sel:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
.stat {
  font-size: 12.5px;
  color: var(--color-text-2);
}
.stat.dim {
  color: var(--color-text-3);
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
.btn {
  padding: 6px 12px;
  border-radius: var(--radius-sm, 8px);
  border: 1px solid var(--color-border);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-size: 12.5px;
  cursor: pointer;
}
.btn:hover:not(:disabled) {
  border-color: var(--color-primary);
  color: var(--color-primary-text);
}
.btn-sm {
  padding: 3px 9px;
  font-size: 12px;
}
</style>
