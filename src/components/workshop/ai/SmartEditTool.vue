<script setup lang="ts">
/**
 * 智能修图工具（FEAT-152 扩展 / FEAT-197 链路口径）
 *
 * 一句话意图 → 语言模型理解并规划 → 文生图模型执行 → 1K 出图（可选上采样）。
 *
 * **只用云端模型**（FEAT-197）：本机算子（调色 / 胶片 / 形变 / 磨皮 / 本机超分）
 * 走「工作台 › 自己组合」，不再由规划器混编 —— 混编会让「按模型计费」的口径失真，
 * 也会把一句话出片变成本机长等待。所以旧的「优先本地组件」开关已删除。
 */
import { computed, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { useAiBilling } from "../../../composables/useAiBilling";
import { dirName, joinPath } from "../../../utils/pathParts";
import {
  MODEL_STYLES,
  switchStyleText,
  toggleStyle,
} from "../../../utils/modelStylePresets";
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
const rounds = ref(1); // 固定 1 轮：界面给了「多轮」但每轮都是计费调用
/** 文生图采样步数（FEAT-197）：默认 1，默认档不写进请求体 */
const renderSteps = ref(1);
/** 上采样：默认不勾（每张多一次云端调用） */
const upsample = ref(false);
const upsampleScale = ref(2);
/** 影调风格预设（FEAT-198）：选中即把中文原文接在提示词后面（可见可编辑） */
const styleId = ref("");

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

/**
 * 选中／取消一款影调风格：把中文原文接进提示词（可见可编辑），并记下 styleId
 * 让后端补英文要点。再点同款 = 取消；用户已手改过那段文字时不擅自删。
 */
function pickStyle(id: string): void {
  if (busy.value) return;
  const next = toggleStyle(styleId.value, id);
  if (next === styleId.value) return;
  prompt.value = switchStyleText(prompt.value, styleId.value, next);
  styleId.value = next;
}
const canRun = computed(() => props.paths.length > 0 && !busy.value);

/* ---------- 花费预估（图像理解 + 图像编辑两个能力位分别计价） ---------- */
const visionPrice = computed(() => billing.modelPrice("vision"));
const editPrice = computed(() => billing.modelPrice("edit"));
const upscalePrice = computed(() => billing.modelPrice("upscale"));

/** 每张预计调用次数：规划 1 次 vision；执行 1 次 edit（规划出的步数在跑之前未知，
 *  按最少 1 次估下限）；上采样 N 遍超分；反思每轮 +1 vision(复核) */
function expectedCalls() {
  const v = 1 + (reflect.value ? rounds.value : 0);
  const e = 1 + (reflect.value ? Math.max(0, rounds.value - 1) : 0);
  const u = upsample.value ? (upsampleScale.value === 4 ? 2 : 1) : 0;
  return { v, e, u };
}

const estimateText = computed(() => {
  const n = props.paths.length;
  const { v, e, u } = expectedCalls();
  const configured =
    visionPrice.value > 0 || editPrice.value > 0 || (u > 0 && upscalePrice.value > 0);
  if (!configured) return "未配置模型单价，无法预估花费（可在「模型设置 › 计费」填写）";
  const total =
    (v * visionPrice.value + e * editPrice.value + u * upscalePrice.value) * n;
  const parts = [`图像理解 ${v} 次`, `图像编辑 ≥${e} 次`];
  if (u) parts.push(`超分 ${u} 次`);
  return `预估 ≈ ${billing.formatMoney(total)}（${n} 张 × [${parts.join(" + ")}]）`;
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

/** 规划器可调用的组件清单（与 `python/ai_studio/presets.py` 的 CLOUD_OP_CATALOG 对齐） */
const COMPONENTS = [
  { name: "人物美颜", kind: "云端" },
  { name: "调色 / 滤镜迁移", kind: "云端" },
  { name: "画质增强", kind: "云端" },
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
      options: {
        reflect: reflect.value,
        rounds: rounds.value,
        steps: renderSteps.value,
        upsample: upsample.value,
        upsampleScale: upsampleScale.value,
        styleId: styleId.value,
      },
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
        <label class="ai-label">影调风格（可选）</label>
        <div class="cmp-list">
          <button
            v-for="s in MODEL_STYLES"
            :key="s.id"
            type="button"
            class="sty-chip"
            :class="{ on: styleId === s.id }"
            :disabled="busy"
            :title="s.zh"
            @click="pickStyle(s.id)"
          >
            {{ s.label }}
          </button>
          <button
            v-if="styleId"
            type="button"
            class="sty-chip"
            :disabled="busy"
            @click="pickStyle('')"
          >
            清空风格
          </button>
        </div>
        <p class="ai-note" style="margin: 6px 0 0">
          选中后会把该风格的描述接在「提示词」后面（可直接改）。它描述影调与光线，不改构图与物体形状。
        </p>
      </div>

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
        <p class="ai-note" style="margin: 6px 0 0">
          这条路线只用云端模型；想用本机免费组件（调色 / 胶片 / 磨皮 / 本机超分），
          到工作台切「自己组合」。
        </p>
      </div>

      <div class="ai-field">
        <label class="ai-label">文生图步数 {{ renderSteps }}</label>
        <input
          v-model.number="renderSteps"
          type="range"
          min="1"
          max="30"
          step="1"
          style="width: 100%"
          :disabled="busy"
        />
        <p class="ai-note" style="margin: 6px 0 8px">
          采样步数，越大越细但越慢越贵；默认 1（不额外声明，用网关默认值）。
        </p>

        <label class="ai-label ai-check">
          <input v-model="upsample" type="checkbox" :disabled="busy" />
          上采样（出图后再放大）
        </label>
        <div class="ai-seg" style="margin-top: 6px">
          <button
            v-for="s in [2, 4]"
            :key="s"
            type="button"
            class="ai-seg-btn"
            :class="{ on: upsampleScale === s }"
            :disabled="!upsample || busy"
            @click="upsampleScale = s"
          >
            {{ s }}×
          </button>
        </div>
        <p class="ai-note" style="margin: 6px 0 8px">
          默认 2×；4× 用 2× 跑两遍（细节更实，费用也是两倍）。
        </p>
      </div>

      <div class="ai-field">
        <label class="ai-label ai-check">
          <input v-model="reflect" type="checkbox" :disabled="busy" />
          反思（出图后自检一轮）
        </label>
        <p class="ai-note" style="margin: 6px 0 8px">
          不勾选（默认）：按计划修一版出图。勾选后：复核一轮，不通过就再修一次。
        </p>
      </div>
    </div>

    <p class="ai-note">
      智能修图先用「图像理解」模型理解意图并规划步骤，再用「图像编辑」模型逐步执行；只输出终图 JPG，不覆盖原图。链路是：原图 → 短边 1080 → 规划 → 执行 → 1K 出图（可选上采样）。
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

/* 风格 chip：与工作台的 `.chip` 同一套观感，避免两处入口长得不一样 */
.sty-chip {
  display: inline-flex;
  align-items: center;
  height: 24px;
  padding: 0 9px;
  border: none;
  border-radius: var(--r-pill);
  box-shadow: inset 0 0 0 1px var(--c-rule);
  background: none;
  color: var(--fg-2);
  font: inherit;
  font-size: 11.5px;
  cursor: pointer;
}
.sty-chip:hover:not(:disabled) {
  color: var(--fg);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 45%, transparent);
}
.sty-chip.on {
  color: var(--fg);
  box-shadow: inset 0 0 0 1px var(--c-accent);
}
.sty-chip:disabled {
  opacity: 0.45;
  cursor: default;
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
