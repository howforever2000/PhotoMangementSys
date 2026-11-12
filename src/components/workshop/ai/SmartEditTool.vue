<script setup lang="ts">
/**
 * 智能修图工具（FEAT-152 扩展）
 * 一句话意图 → 多模态模型规划步骤 → 执行修图；可选「反思」多轮复核修正；只输出终图。
 * 执行过程用小会话框展示「正在做哪一步 + 预估花费」。
 */
import { computed, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { useAiBilling } from "../../../composables/useAiBilling";
import { dirName, joinPath } from "../../../utils/pathParts";
import AiPromptPanel from "./AiPromptPanel.vue";
import AiResultPanel from "./AiResultPanel.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();
const billing = useAiBilling();

const prompt = ref("");
const outputDir = ref("");
const outputTouched = ref(false);
const reflect = ref(false); // 默认不勾选反思
const rounds = ref(1); // 默认 1 轮

watch(
  () => props.paths,
  (paths) => {
    if (outputTouched.value) return;
    const first = paths[0];
    const dir = first ? dirName(first) : "";
    outputDir.value = dir ? joinPath(dir, "_ai") : "";
  },
  { immediate: true },
);

// 作业归属提在 store（activeByTool）：切走页面/工具再回来能按工具名取回作业重连
const job = computed(() => store.jobOfTool("smart_edit"));
const busy = computed(
  () => !!job.value && (job.value.status === "queued" || job.value.status === "running"),
);
const canRun = computed(() => props.paths.length > 0 && !busy.value);

/* ---------- 花费预估（图像理解 + 图像编辑两个能力位分别计价） ---------- */
const visionPrice = computed(() => billing.modelPrice("vision"));
const editPrice = computed(() => billing.modelPrice("edit"));
const upscalePrice = computed(() => billing.modelPrice("upscale"));

/** 每张预计调用次数：规划 1 次 vision；执行 1 次 edit；反思每轮 +1 vision(复核)、额外 +（轮次-1）edit(微调) */
function expectedCalls() {
  const v = 1 + (reflect.value ? rounds.value : 0);
  const e = 1 + (reflect.value ? Math.max(0, rounds.value - 1) : 0);
  return { v, e };
}

const estimateText = computed(() => {
  const n = props.paths.length;
  const { v, e } = expectedCalls();
  const configured = visionPrice.value > 0 || editPrice.value > 0;
  if (!configured) return "未配置模型单价，无法预估花费（可在「模型设置 › 计费」填写）";
  const total = (v * visionPrice.value + e * editPrice.value) * n;
  return `预估 ≈ ${billing.formatMoney(total)}（${n} 张 × [图像理解 ${v} 次 + 图像编辑 ${e} 次]）`;
});

/** 运行结束后按后端 meta.stats 上报的实际调用次数计价 */
const actualText = computed(() => {
  const meta = job.value?.meta as Record<string, unknown> | undefined;
  const s = meta?.stats as
    | { visionCalls?: number; editCalls?: number; upscaleCalls?: number }
    | undefined;
  if (!s) return "";
  if (visionPrice.value <= 0 && editPrice.value <= 0 && upscalePrice.value <= 0) return "";
  const total =
    (s.visionCalls || 0) * visionPrice.value +
    (s.editCalls || 0) * editPrice.value +
    (s.upscaleCalls || 0) * upscalePrice.value;
  return `实际调用：图像理解 ${s.visionCalls || 0} 次 · 图像编辑 ${s.editCalls || 0} 次 · 超分 ${
    s.upscaleCalls || 0
  } 次，花费 ≈ ${billing.formatMoney(total)}`;
});

/* ---------- 执行会话框：把阶段变化记成日志 ---------- */
const logs = ref<string[]>([]);
watch(
  () => [job.value?.status, job.value?.stage, job.value?.progress, job.value?.message] as const,
  () => {
    const j = job.value;
    if (!j) return;
    let line = "";
    if (j.status === "queued") line = "· 排队中…";
    else if (j.status === "running")
      line = `· ${j.stage || "处理中"} ${j.progress}%${j.message ? " · " + j.message : ""}`;
    else if (j.status === "done") line = "✓ 完成";
    else if (j.status === "failed") line = `✗ 失败：${j.error || ""}`;
    else if (j.status === "cancelled") line = "已取消";
    if (!line) return;
    if (logs.value[logs.value.length - 1] === line) return; // 去掉相邻重复
    logs.value.push(line);
    if (logs.value.length > 200) logs.value.splice(0, logs.value.length - 200);
  },
);

function resetLogs() {
  logs.value = [];
}

async function run() {
  if (!outputDir.value) {
    notify.warning("还没有输出目录", "先选择修图结果要放在哪个文件夹");
    return;
  }
  resetLogs();
  try {
    await store.run({
      tool: "smart_edit",
      paths: props.paths,
      prompt: prompt.value,
      outputDir: outputDir.value,
      options: { reflect: reflect.value, rounds: rounds.value },
    });
  } catch (e) {
    notify.error("提交失败", String(e));
  }
}

function onDir(v: string) {
  outputTouched.value = true;
  outputDir.value = v;
}

async function onCancel() {
  const id = store.activeJobId("smart_edit");
  if (id) await store.cancel(id);
}
</script>

<template>
  <section class="ai-tool">
    <div class="ai-panel">
      <AiPromptPanel
        :prompt="prompt"
        :output-dir="outputDir"
        :disabled="busy"
        :rows="4"
        placeholder="用一句话描述想要的效果，例如：把这张海边照片调成通透的日系清新风，提亮暗部、降低饱和…"
        @update:prompt="prompt = $event"
        @update:outputDir="onDir"
      />
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <div class="ai-field">
        <label class="ai-label ai-check">
          <input v-model="reflect" type="checkbox" :disabled="busy" />
          反思（多轮复核修正）
        </label>
        <p class="ai-note" style="margin: 6px 0 8px">
          不勾选（默认）：按计划修一版出图。勾选后：每轮复核，通过即停，最多到设定的轮次。
        </p>
        <div class="ai-seg">
          <button
            v-for="r in [1, 2, 3, 4]"
            :key="r"
            type="button"
            class="ai-seg-btn"
            :class="{ on: rounds === r }"
            :disabled="!reflect || busy"
            @click="rounds = r"
          >
            {{ r }} 轮
          </button>
        </div>
      </div>
    </div>

    <p class="ai-note">
      智能修图先用「图像理解」模型理解意图并规划步骤，再用「图像编辑」模型逐步执行；只输出终图 JPG，不覆盖原图。开启反思时，会把上一版结果连同问题点再交给模型修正。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "处理中…" : `开始修图（${paths.length} 张）` }}
      </button>
    </div>

    <p class="ai-hint ai-billing-hint">💡 {{ estimateText }}</p>

    <!-- 执行会话框 -->
    <section v-if="job" class="se-console">
      <div class="se-console-head">
        <strong>执行过程</strong>
        <span class="se-cost">{{ actualText || estimateText }}</span>
      </div>
      <div class="se-log">
        <div v-for="(l, i) in logs" :key="i" class="se-log-line">{{ l }}</div>
        <div v-if="!logs.length" class="se-log-line dim">等待开始…</div>
      </div>
    </section>

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>

<style scoped>
.ai-check {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  cursor: pointer;
}

.ai-check input {
  accent-color: var(--c-accent);
  width: 14px;
  height: 14px;
}

.se-console {
  margin-top: 12px;
  border: 1px solid var(--c-rule);
  border-radius: var(--r-ui);
  overflow: hidden;
}

.se-console-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  background: var(--hover, rgba(120, 120, 130, 0.05));
  border-bottom: 1px solid var(--c-rule);
  font-size: var(--fs-meta);
}

.se-cost {
  margin-left: auto;
  color: var(--fg-2);
  text-align: right;
}

.se-log {
  max-height: 180px;
  overflow: auto;
  padding: 8px 12px;
  font-size: var(--fs-meta);
  line-height: 1.7;
  font-variant-numeric: tabular-nums;
}

.se-log-line {
  color: var(--fg-2);
  white-space: pre-wrap;
  word-break: break-word;
}

.se-log-line.dim {
  opacity: 0.7;
}
</style>
