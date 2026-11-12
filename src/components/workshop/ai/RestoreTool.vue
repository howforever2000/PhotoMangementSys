<script setup lang="ts">
/**
 * 画质修复（FEAT-163）
 * Real-ESRGAN 2×/4× 超分 + 可选 GFPGAN 人脸修复，纯本机推理，输出高清 JPG 副本。
 * 大模型需用户按「本地模型」面板的链接下载；缺失时明确提示，不假跑。
 */
import { computed, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { dirName, joinPath } from "../../../utils/pathParts";
import { RESTORE_SCALES } from "../../../utils/aiPresets";
import OutputDirField from "./OutputDirField.vue";
import LocalModelHint from "./LocalModelHint.vue";
import AiResultPanel from "./AiResultPanel.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

const scale = ref("4");
const face = ref(true);
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

const job = computed(() => store.jobOfTool("restore"));
const busy = computed(
  () => !!job.value && (job.value.status === "queued" || job.value.status === "running"),
);
const canRun = computed(() => props.paths.length > 0 && !busy.value);

async function run() {
  if (!outputDir.value) {
    notify.warning("还没有输出目录", "先选择结果要放在哪个文件夹");
    return;
  }
  try {
    await store.run({
      tool: "restore",
      paths: props.paths,
      prompt: "",
      outputDir: outputDir.value,
      options: { scale: Number(scale.value) || 4, face: face.value },
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
  const id = store.activeJobId("restore");
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
            v-for="s in RESTORE_SCALES"
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

      <label class="ai-row" style="gap: 6px; cursor: pointer">
        <input v-model="face" type="checkbox" :disabled="busy" />
        <span class="ai-label" style="letter-spacing: normal">同时修复人脸（GFPGAN）</span>
      </label>
    </div>

    <div style="margin-top: 12px">
      <LocalModelHint :ids="['real_esrgan_x4', 'gfpgan_1.4']" />
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <OutputDirField :output-dir="outputDir" :disabled="busy" @update:output-dir="onDir" />
    </div>

    <p class="ai-note">
      用 Real-ESRGAN 放大并重建细节，可选对人脸做 GFPGAN 修复；全程本机 GPU/CPU 推理、不上传、免费。输出高清 JPG 副本，不覆盖原图。原图很大时（长边超过 2000px）会先适度缩小再放大，以控制体积。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "处理中…" : `开始修复（${paths.length} 张）` }}
      </button>
    </div>

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>
