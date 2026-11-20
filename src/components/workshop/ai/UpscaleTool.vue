<script setup lang="ts">
/**
 * 画质增强 / 超分工具（FEAT-152 → 切本机）
 * Real-ESRGAN 2×/4× 放大 + 去噪，纯本机推理，输出高清 JPG 副本。
 * 与「画质修复」的分工：本工具不做 GFPGAN 人脸修复（那是修复工具的事）。
 */
import { computed, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { dirName, joinPath } from "../../../utils/pathParts";
import { UPSCALE_SCALES } from "../../../utils/aiPresets";
import OutputDirField from "./OutputDirField.vue";
import LocalModelHint from "./LocalModelHint.vue";
import AiResultPanel from "./AiResultPanel.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

const scale = ref("2");
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
      prompt: "",
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

    <div style="margin-top: 12px">
      <LocalModelHint :ids="['real_esrgan_x4']" />
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <OutputDirField :output-dir="outputDir" :disabled="busy" @update:output-dir="onDir" />
    </div>

    <p class="ai-note">
      用 Real-ESRGAN 放大并重建细节；全程本机 GPU/CPU 推理、不上传、免费。输出高清 JPG 副本，不覆盖原图。原图很大时（长边超过 2000px）会先适度缩小再放大，以控制体积。需要修复人脸请用「画质修复」。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "处理中…" : `开始增强（${paths.length} 张）` }}
      </button>
    </div>

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>
