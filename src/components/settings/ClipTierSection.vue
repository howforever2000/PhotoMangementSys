<script setup lang="ts">
/**
 * ② 语义模型（档位）—— 性能设置的第二段
 *
 * 只保留「与机器适配有关」的选择；模型下载 / 镜像源治理属于**网络**，不在此处
 * （见同级 ModelManageSection，由「模型管理」页签单独提供）。
 *
 * 本组件同时是「识别服务是否可用」的探针：`list_vcr_models` 是最轻、又必然依赖
 * 服务的调用。它失败即代表服务没起来 —— 由父组件据此显示横幅并锁定其余分区
 * （BUG-2026-0918-002：不可用时**锁定而非整块隐藏**）。
 */
import { computed, onMounted, ref, watch } from "vue";
import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import { PERF_TIMEOUT, withTimeout } from "../../utils/withTimeout";
import ConfirmDialog from "../ConfirmDialog.vue";

const props = defineProps<{
  /** 父级服务不可用 → 本区锁定 */
  locked?: boolean;
  /** 父级「重试」时自增，触发重新加载（不用函数当 prop） */
  retryToken?: number;
}>();
const emit = defineEmits<{
  /** 识别服务不可用（父组件显示横幅 + 锁定其余分区） */
  failed: [msg: string];
  /** 服务恢复 */
  recovered: [];
}>();

const store = useContentStore();
const notify = useNotify();

const loading = ref(true);
const failed = ref(false);
const errMsg = ref("");
const busy = ref(false);
const selected = ref("");
/** 切换前需二次确认：换档会作废旧的语义索引（破坏性操作） */
const confirmOpen = ref(false);

const info = computed(() => store.vcrModels);
const active = computed(() => info.value?.models?.find((m) => m.active) ?? null);
/** 当前生效档位的一句话规格（维数 · 输入尺寸 · 体积） */
const tip = computed(() => {
  const m = active.value;
  if (!m) return "";
  const mb = m.bytes ? ` · ${(m.bytes / 1e6).toFixed(0)}MB` : "";
  return `${m.dim} 维 · 输入 ${m.size}×${m.size}${mb}`;
});

async function load(silent = false) {
  loading.value = true;
  failed.value = false;
  try {
    const v = await withTimeout(store.fetchVcrModels(), PERF_TIMEOUT.read, "list_vcr_models");
    selected.value = v.current ?? "";
    errMsg.value = "";
    emit("recovered");
    return true;
  } catch (e) {
    if (!silent) {
      failed.value = true;
      errMsg.value = String(e);
      emit("failed", String(e));
    }
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

/** 用户改下拉 → 先弹二次确认，确认后才真正切档 */
function onChange() {
  confirmOpen.value = true;
}
function onCancel() {
  confirmOpen.value = false;
  // 回显真实值：服务端没改成功就绝不能显示成已切换
  selected.value = info.value?.current ?? selected.value;
}
async function onConfirm() {
  confirmOpen.value = false;
  const name = selected.value;
  busy.value = true;
  try {
    const v = await withTimeout(store.setVcrModel(name), PERF_TIMEOUT.write, "set_vcr_model");
    selected.value = v.current ?? name;
    notify.success(
      "语义模型已切换",
      v.clip_ready === false
        ? `${name} · 后台加载中，就绪后自动生效`
        : `${name} · ⚠ 需重建语义索引：对相册重扫一次含「语义向量」的扫描，或到「内容分类」页点「🔄 重建分类」（未重建前语义搜索/分类会提示尚未建立索引）`,
    );
  } catch (e) {
    notify.error("切换模型失败", String(e));
    void load(true);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <section class="sec">
    <div class="sec-head">
      <h3 class="sec-title">语义模型</h3>
      <span class="sec-sub">档位决定向量空间；换档后必须重建语义索引</span>
    </div>

    <div class="sec-row">
      <label class="lbl" for="clip-tier">模型档位</label>
      <select
        id="clip-tier"
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
          {{ m.label }}{{ m.bytes ? ` · ${(m.bytes / 1e6).toFixed(0)}MB` : "" }}{{
            m.downloaded ? "" : "（未下载）"
          }}{{ m.active ? " ✓当前" : "" }}
        </option>
      </select>

      <span v-if="busy || loading" class="stat dim">切换中…</span>
      <span v-else-if="failed" class="stat err">
        清单加载失败
        <button class="btn btn-sm" @click="load()">重试</button>
      </span>
      <span v-else-if="info?.clip_ready === false" class="stat dim">模型加载中…</span>
      <span v-else-if="tip" class="stat">{{ tip }}</span>
    </div>

    <p v-if="active?.note" class="hint">{{ active.note }}</p>
    <p class="hint">
      <b>默认档（B/16）即推荐档</b>：可核显本、CPU 即可跑。切到
      <b>B/16 fp32</b> 是为了用 GPU 加速语义索引，但本机实测——它与识别同时跑会抢同一把
      GPU 队列，两腿合计反而变慢（<code>166.5</code> vs <code>141.8</code> ms/张），
      除非只做语义索引不跑识别，否则不建议换。
    </p>

    <ConfirmDialog
      :visible="confirmOpen"
      title="切换语义模型档位？"
      message="换档会作废当前语义索引：新旧向量不在同一空间，不会被混用。切换后必须对相册重新扫描一次「语义向量」，或到「内容分类」页点「重建分类」，否则语义搜索/分类会提示「尚未建立索引」。"
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
  min-width: 210px;
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
.hint code {
  background: var(--color-neutral-soft);
  padding: 1px 5px;
  border-radius: 4px;
  font-family: Consolas, monospace;
  font-size: 11.5px;
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
  color: var(--color-primary);
}
.btn:focus-visible {
  outline: 2px solid rgba(57, 108, 216, 0.55);
  outline-offset: 2px;
}
.btn:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}
.btn-sm {
  padding: 3px 9px;
  font-size: 12px;
}
</style>
