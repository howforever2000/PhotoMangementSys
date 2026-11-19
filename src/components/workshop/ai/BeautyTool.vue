<script setup lang="ts">
/**
 * 人物美颜工具（FEAT-152 → FEAT-169）
 * 预设风格 × 级别 + 可选提示词 → 逐张美颜，输出 JPG。
 *
 * ## 形状纪律（FEAT-169）
 *
 * 用户规则：「美颜保持当前人体形状不变，只做分区磨皮和细节」。这个工具走的是
 * 云端整幅重绘，**不可能**像本机分区磨皮那样给出数学保证，所以：
 *   1. 后端提示词只描述皮肤质感并强制追加结构保全硬段；
 *   2. 后端出图后用 `critic.structure_compare` 校验，检出改形状就回退原图并告警；
 *   3. 面板上直接给出「要绝对不变走本地美颜」的直达入口，不把判断留给用户猜。
 */
import { computed, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { dirName, joinPath } from "../../../utils/pathParts";
import { BEAUTY_LEVELS, BEAUTY_STYLES } from "../../../utils/aiPresets";
import { STRUCTURE_LOCK_ZH } from "../../../utils/structureLock";
import { localBeautyTarget, routeToLocalTool } from "../../../composables/useLocalToolHandoff";
import AiPromptPanel from "./AiPromptPanel.vue";
import AiResultPanel from "./AiResultPanel.vue";
import AiCostHint from "./AiCostHint.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

const style = ref("natural");
const level = ref("medium");
const prompt = ref("");
const outputDir = ref("");
const outputTouched = ref(false);

/** 「要形状绝对不变」的直接出口：切到本机分区磨皮，并把素材一起带过去 */
const goLocal = routeToLocalTool(localBeautyTarget());

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
const job = computed(() => store.jobOfTool("beauty"));
const busy = computed(
  () => !!job.value && (job.value.status === "queued" || job.value.status === "running"),
);
const canRun = computed(() => props.paths.length > 0 && !busy.value);

async function run() {
  if (!outputDir.value) {
    notify.warning("还没有输出目录", "先选择美颜结果要放在哪个文件夹");
    return;
  }
  try {
    await store.run({
      tool: "beauty",
      paths: props.paths,
      prompt: prompt.value,
      outputDir: outputDir.value,
      options: { style: style.value, level: level.value },
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
  const id = store.activeJobId("beauty");
  if (id) await store.cancel(id);
}
</script>

<template>
  <section class="ai-tool">
    <div class="ai-panel ai-2col">
      <div class="ai-field">
        <label class="ai-label">美颜风格</label>
        <div class="ai-seg">
          <button
            v-for="s in BEAUTY_STYLES"
            :key="s.value"
            type="button"
            class="ai-seg-btn"
            :class="{ on: style === s.value }"
            @click="style = s.value"
          >
            {{ s.label }}
          </button>
        </div>
      </div>
      <div class="ai-field">
        <label class="ai-label">级别</label>
        <div class="ai-seg">
          <button
            v-for="l in BEAUTY_LEVELS"
            :key="l.value"
            type="button"
            class="ai-seg-btn"
            :class="{ on: level === l.value }"
            @click="level = l.value"
          >
            {{ l.label }}
          </button>
        </div>
      </div>
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <AiPromptPanel
        :prompt="prompt"
        :output-dir="outputDir"
        :disabled="busy"
        placeholder="补充要求（可选），例如：保留皮肤真实质感、不要过度磨皮…"
        @update:prompt="prompt = $event"
        @update:outputDir="onDir"
      />
    </div>

    <p class="ai-note">
      只按所选风格与级别调整肤质、瑕疵与光线，<strong class="lock">{{ STRUCTURE_LOCK_ZH }}</strong>
      每张出图后会做结构校验，检出脸型或体型被改会改用原图并提示。
      要绝对保证形状不变，用下面这条路。
    </p>

    <p class="ai-hint-row">
      <button class="ai-ghost" type="button" :disabled="busy" @click="goLocal(props.paths)">
        改用本地美颜（只磨皮肤，形状不动、免费）
      </button>
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "处理中…" : `开始美颜（${paths.length} 张）` }}
      </button>
    </div>

    <AiCostHint tool="beauty" :count="paths.length" :settings="store.settings" />

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>

<style scoped>
.lock {
  font-weight: 600;
  color: var(--fg);
}
.ai-hint-row {
  margin: 10px 0 0;
}
</style>
