<script setup lang="ts">
/**
 * ① 本机硬件 · CPU 线程数（ONNX intra_op，不同机器最优值不同）
 *
 * 数据读 Pinia `vcrThreads`：③ 区的「线程测速 → 采用」写的是同一份 store，
 * 这里用 watch 同步本地下拉值，两处显示不会出现「改了一处另一处还是旧值」。
 */
import { computed, onMounted, ref, watch } from "vue";
import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import { PERF_TIMEOUT, withTimeout } from "../../utils/withTimeout";

defineProps<{ locked?: boolean }>();

const store = useContentStore();
const notify = useNotify();

const threads = computed(() => store.vcrThreads);
const threadsSel = ref(0);
const threadsBusy = ref(false);
const threadsFailed = ref(false);

async function loadThreads() {
  try {
    const info = await withTimeout(store.fetchVcrThreads(), PERF_TIMEOUT.read, "get_vcr_threads");
    if (!threadsSel.value) threadsSel.value = info.threads;
    threadsFailed.value = false;
    return true;
  } catch {
    threadsFailed.value = true;
    return false;
  }
}

onMounted(() => void loadThreads());

// ③ 区的「采用最快档」也会改线程数：store 变了就同步到下拉
watch(
  () => threads.value?.threads,
  (v) => {
    if (v) threadsSel.value = v;
  },
);

async function onThreadsChange() {
  threadsBusy.value = true;
  try {
    const info = await withTimeout(
      store.setVcrThreads(threadsSel.value),
      PERF_TIMEOUT.write,
      "set_vcr_threads",
    );
    threadsSel.value = info.threads;
    notify.success(
      `CPU 线程数已设为 ${info.threads}`,
      `默认按物理核推测为 ${info.default}（本机逻辑核 ${info.logical}）；新设置对后续推理立即生效`,
    );
  } catch (e) {
    notify.error("设置线程数失败", String(e));
    threadsSel.value = threads.value?.threads ?? 0;
    void loadThreads();
  } finally {
    threadsBusy.value = false;
  }
}

const threadsInfoText = computed(() => {
  const t = threads.value;
  if (!t) return "";
  return `生效 ${t.threads} 线程 · 物理核约 ${t.physical_guess}（逻辑 ${t.logical}）`;
});
</script>

<template>
  <div class="sec-row">
    <label class="lbl" for="cpu-threads">CPU 线程数</label>
    <select
      id="cpu-threads"
      v-model.number="threadsSel"
      class="sel"
      :disabled="locked || threadsBusy || !threads"
      @change="onThreadsChange"
    >
      <option v-for="n in threads?.options ?? []" :key="n" :value="n">
        {{ n }} 线程{{ n === threads?.default ? "（推荐）" : "" }}
      </option>
    </select>
    <span v-if="threadsBusy" class="stat dim">切换中…</span>
    <span v-else-if="threadsFailed" class="stat err">
      加载失败
      <button class="btn btn-sm" @click="loadThreads()">重试</button>
    </span>
    <span v-else-if="threads" class="stat">{{ threadsInfoText }}</span>
  </div>
  <p class="hint">
    推理线程数只影响<b>识别速度</b>：默认取物理核数（超线程的逻辑核收益低）。
    想确认本机最优档，到下面的「高级选项 → 线程测速」跑一次。
  </p>
</template>

<style scoped>
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
  min-width: 150px;
  padding: 7px 10px;
  border-radius: var(--radius-sm, 8px);
  border: 1px solid var(--color-border);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-size: 13px;
}
.sel:focus-visible {
  outline: 2px solid rgba(57, 108, 216, 0.55);
  outline-offset: 1px;
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
.btn {
  min-height: 26px;
  padding: 3px 10px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm, 8px);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-size: 12px;
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
.hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--color-text-3);
}
</style>
