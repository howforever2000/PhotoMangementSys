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
/**
 * 优先本地组件（FEAT-164）：勾上后规划提示词追加偏好，让模型尽量用 local_*
 * （胶片 LUT / 本地超分）完成，减少外部模型调用。默认不勾 —— 本地组件省调用，
 * 但表现力弱于云端编辑模型，交给用户按场景选。
 */
const localFirst = ref(false);

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
  const tail = localFirst.value ? "；勾了优先本地组件，实际外部调用可能更少" : "";
  return `预估 ≈ ${billing.formatMoney(total)}（${n} 张 × [图像理解 ${v} 次 + 图像编辑 ${e} 次]）${tail}`;
});

/** 运行结束后按后端 meta.stats 上报的实际调用次数计价（本地组件 0 元） */
const actualText = computed(() => {
  const meta = job.value?.meta as Record<string, unknown> | undefined;
  const s = meta?.stats as
    | { visionCalls?: number; editCalls?: number; upscaleCalls?: number; localCalls?: number }
    | undefined;
  if (!s) return "";
  const local = s.localCalls || 0;
  if (visionPrice.value <= 0 && editPrice.value <= 0 && upscalePrice.value <= 0) return "";
  const total =
    (s.visionCalls || 0) * visionPrice.value +
    (s.editCalls || 0) * editPrice.value +
    (s.upscaleCalls || 0) * upscalePrice.value;
  return `实际调用：图像理解 ${s.visionCalls || 0} 次 · 图像编辑 ${s.editCalls || 0} 次 · 超分 ${
    s.upscaleCalls || 0
  } 次 · 本地组件 ${local} 次（免费），花费 ≈ ${billing.formatMoney(total)}`;
});

/** 规划器可调用的组件清单（与 python/ai_studio/presets.py 的 OP_CATALOG 对齐） */
const COMPONENTS = [
  { name: "人物美颜", kind: "云端" },
  { name: "调色 / 滤镜迁移", kind: "云端" },
  { name: "画质增强", kind: "云端" },
  { name: "胶片模拟（内置 LUT）", kind: "本机" },
  { name: "本地超分（Real-ESRGAN）", kind: "本机" },
];

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
      options: { reflect: reflect.value, rounds: rounds.value, localFirst: localFirst.value },
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
        <label class="ai-label">可调用组件</label>
        <div class="cmp-list">
          <span
            v-for="c in COMPONENTS"
            :key="c.name"
            class="cmp-chip"
            :class="{ local: c.kind === '本机' }"
          >
            {{ c.name }}<em>{{ c.kind }}</em>
          </span>
        </div>
        <label class="ai-label ai-check" style="margin-top: 8px">
          <input v-model="localFirst" type="checkbox" :disabled="busy" />
          优先本地组件（免费、离线；表现力略弱于云端模型）
        </label>
      </div>

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
      智能修图先用「图像理解」模型理解意图并规划步骤，再用「图像编辑」模型逐步执行；只输出终图 JPG，不覆盖原图。规划器也可以调用本机组件（内置胶片 LUT、本地 Real-ESRGAN 超分），这些步骤免费且不把照片发出去；开启反思时，会把上一版结果连同问题点再交给模型修正。
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

/* 组件清单：本机组件用金色描边区分（它们免费、不过网，是需要被看见的那一类） */
.cmp-list {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 6px;
}
.cmp-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 24px;
  padding: 0 8px;
  border-radius: var(--r-pill);
  box-shadow: inset 0 0 0 1px var(--c-rule);
  font-size: 11.5px;
  color: var(--fg-2);
}
.cmp-chip.local {
  color: var(--fg);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 45%, transparent);
}
.cmp-chip em {
  font-style: normal;
  font-size: 10px;
  color: var(--c-accent);
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
