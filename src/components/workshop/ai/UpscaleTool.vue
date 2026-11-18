<script setup lang="ts">
/**
 * 画质增强 / 超分工具（FEAT-152）
 * 2× / 4× 放大 + 可选提示词 → 逐张超分，输出 JPG。
 */
import { computed, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { dirName, joinPath } from "../../../utils/pathParts";
import { UPSCALE_SCALES } from "../../../utils/aiPresets";
import AiPromptPanel from "./AiPromptPanel.vue";
import AiResultPanel from "./AiResultPanel.vue";
import AiCostHint from "./AiCostHint.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

const scale = ref("2");
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

// 作业归属提在 store（activeByTool）：切走页面/工具再回来能按工具名取回作业重连
const job = computed(() => store.jobOfTool("upscale"));
const busy = computed(
  () => !!job.value && (job.value.status === "queued" || job.value.status === "running"),
);
const canRun = computed(() => props.paths.length > 0 && !busy.value);

async function run() {
  if (!outputDir.value) {
    notify.warning("还没有输出目录", "先选择增强结果要放在哪个文件夹");
    return;
  }
  try {
    await store.run({
      tool: "upscale",
      paths: props.paths,
      prompt: prompt.value,
      outputDir: outputDir.value,
      options: { scale: Number(scale.value) || 2 },
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
  const id = store.activeJobId("upscale");
  if (id) await store.cancel(id);
}
</script>

<template>
  <section class="ai-tool">
    <div class="ai-panel">
      <div class="ai-field">
        <label class="ai-label">放大倍数</label>
        <div class="ai-seg">
          <button
            v-for="s in UPSCALE_SCALES"
            :key="s.value"
            type="button"
            class="ai-seg-btn"
            :class="{ on: scale === s.value }"
            @click="scale = s.value"
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
        placeholder="补充要求（可选），例如：重点还原人脸细节、去掉压缩噪点…"
        @update:prompt="prompt = $event"
        @update:outputDir="onDir"
      />
    </div>

    <p class="ai-note">
      超分走独立配置的超分模型（未配置时回落图像编辑模型）；输出高清 JPG 副本，不覆盖原图。放大后的实际分辨率取决于服务商模型能力。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "处理中…" : `开始增强（${paths.length} 张）` }}
      </button>
    </div>

    <AiCostHint tool="upscale" :count="paths.length" />

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>
