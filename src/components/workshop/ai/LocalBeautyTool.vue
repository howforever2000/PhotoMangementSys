<script setup lang="ts">
/**
 * 本地美颜 · 分区磨皮（FEAT-165）
 * SCRFD 找人脸 → BiSeNet 分区 → 只磨皮肤 → 羽化贴回。五官/头发/背景像素不动。
 * 纯本机、零费用；需要本地人脸解析模型（应用内下载）。
 */
import { computed, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { dirName, joinPath } from "../../../utils/pathParts";
import { LOCAL_BEAUTY_LEVELS } from "../../../utils/aiPresets";
import OutputDirField from "./OutputDirField.vue";
import LocalModelHint from "./LocalModelHint.vue";
import AiResultPanel from "./AiResultPanel.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

const level = ref("0.6");
const texture = ref(35);
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

const job = computed(() => store.jobOfTool("local_beauty"));
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
      tool: "local_beauty",
      paths: props.paths,
      prompt: "",
      outputDir: outputDir.value,
      options: {
        strength: Number(level.value) || 0.6,
        texture: texture.value / 100,
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
  const id = store.activeJobId("local_beauty");
  if (id) await store.cancel(id);
}
</script>

<template>
  <section class="ai-tool">
    <div class="ai-panel">
      <div class="ai-field">
        <label class="ai-label">磨皮力度</label>
        <div class="ai-seg">
          <button
            v-for="s in LOCAL_BEAUTY_LEVELS"
            :key="s.value"
            type="button"
            class="ai-seg-btn"
            :class="{ on: level === s.value }"
            :disabled="busy"
            @click="level = s.value"
          >
            {{ s.label }}
          </button>
        </div>
      </div>

      <div class="ai-field">
        <label class="ai-label">保留皮肤质感 {{ texture }}%</label>
        <input v-model.number="texture" type="range" min="0" max="90" :disabled="busy" />
        <p class="ai-note" style="margin: 4px 0 0">
          越高越接近原皮肤纹理；调到 0 会明显「塑料脸」，不建议。
        </p>
      </div>
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <LocalModelHint :ids="['face_parsing_r34']" />
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <OutputDirField :output-dir="outputDir" :disabled="busy" @update:output-dir="onDir" />
    </div>

    <p class="ai-note">
      只对识别出的皮肤区域做柔化，五官、头发、衣物与背景像素原样保留，分辨率不变；
      全程本机完成；输出为新 JPG，不覆盖原图。未检出人脸的照片会原样输出并提示。
    </p>

    <div class="ai-actions">
      <button class="ai-primary" type="button" :disabled="!canRun" @click="run">
        开始美颜（{{ paths.length }} 张）
      </button>
    </div>

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>
