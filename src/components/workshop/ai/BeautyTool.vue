<script setup lang="ts">
/**
 * 人物美颜工具（FEAT-152）
 * 预设风格 × 级别 + 可选提示词 → 逐张美颜，输出 JPG。
 */
import { computed, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { dirName, joinPath } from "../../../utils/pathParts";
import { BEAUTY_LEVELS, BEAUTY_STYLES } from "../../../utils/aiPresets";
import AiPromptPanel from "./AiPromptPanel.vue";
import AiResultPanel from "./AiResultPanel.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

const style = ref("natural");
const level = ref("medium");
const prompt = ref("");
const outputDir = ref("");
const outputTouched = ref(false);

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

const jobId = ref<string | null>(null);
const job = computed(() => store.jobOf(jobId.value));
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
    jobId.value = await store.run({
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
  if (jobId.value) await store.cancel(jobId.value);
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

    <p class="ai-note">美颜会保留人物身份与五官特征，仅按所选风格与级别调整肤质、光线与整体质感；输出为新 JPG，不覆盖原图。</p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "处理中…" : `开始美颜（${paths.length} 张）` }}
      </button>
    </div>

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>
