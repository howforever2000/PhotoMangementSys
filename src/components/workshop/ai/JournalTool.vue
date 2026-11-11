<script setup lang="ts">
/**
 * 主题手账工具（FEAT-152）
 * 版式模板 × 画面风格 + 提示词 → 生成手账（1 张 JPG / 多张 PDF）。
 */
import { computed, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { dirName, joinPath } from "../../../utils/pathParts";
import { JOURNAL_STYLES, JOURNAL_TEMPLATES } from "../../../utils/aiPresets";
import AiPromptPanel from "./AiPromptPanel.vue";
import AiResultPanel from "./AiResultPanel.vue";
import AiCostHint from "./AiCostHint.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

const template = ref("blank");
const style = ref("sketch");
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
    notify.warning("还没有输出目录", "先选择手账结果要放在哪个文件夹");
    return;
  }
  try {
    jobId.value = await store.run({
      tool: "journal",
      paths: props.paths,
      prompt: prompt.value,
      outputDir: outputDir.value,
      options: { template: template.value, style: style.value },
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
        <label class="ai-label">版式</label>
        <div class="ai-seg">
          <button
            v-for="t in JOURNAL_TEMPLATES"
            :key="t.value"
            type="button"
            class="ai-seg-btn"
            :class="{ on: template === t.value }"
            :title="t.hint"
            @click="template = t.value"
          >
            {{ t.label }}
          </button>
        </div>
      </div>
      <div class="ai-field">
        <label class="ai-label">画面风格</label>
        <div class="ai-seg">
          <button
            v-for="s in JOURNAL_STYLES"
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
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <AiPromptPanel
        :prompt="prompt"
        :output-dir="outputDir"
        :disabled="busy"
        placeholder="例如：以「留白」为主题，把照片做成安静克制的手账；补充要求可写在这里…"
        @update:prompt="prompt = $event"
        @update:outputDir="onDir"
      />
    </div>

    <p class="ai-note">
      1 张照片输出 JPG，多张输出多页 PDF（每张一页，附首图预览）。模型负责提炼主题、写配文与画面风格化；文字与版式由本地模板确定渲染。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "生成中…" : `生成手账（${paths.length} 张）` }}
      </button>
      <span v-if="paths.length > 10" class="ai-hint">张数较多，逐张风格化耗时较长</span>
    </div>

    <AiCostHint tool="journal" :count="paths.length" />

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>
