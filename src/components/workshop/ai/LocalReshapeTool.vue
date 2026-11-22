<script setup lang="ts">
/**
 * 人像形变 · 瘦脸 + 大眼（FEAT-167）
 *
 * SCRFD 找人脸 → peppa_wutz 出 68 点 → RBF 位移场 → 局部液化。
 * 纯本机、零费用；需要人脸检测（扫描侧已有）+ 68 点关键点模型（应用内下载，14MB）。
 *
 * 强度上限刻意保守（瘦脸 0.8 / 大眼 0.5）：再高会明显失真，实测见
 * `python/ai_studio/facewarp.py` 头注释。形变过火**没有其它参数能救回来**，
 * 这与 `knowledge/local-enhance.md` §9「宁可欠一点，不要过火」是同一条纪律。
 */
import { computed, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { dirName, joinPath } from "../../../utils/pathParts";
import { RESHAPE_EYE_LEVELS, RESHAPE_SLIM_LEVELS } from "../../../utils/aiPresets";
import OutputDirField from "./OutputDirField.vue";
import LocalModelHint from "./LocalModelHint.vue";
import AiResultPanel from "./AiResultPanel.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

// 默认档与配方一致（FEAT-210）：自然瘦脸 15%、大眼 20%、眼睛提亮 10%
const slim = ref("0.15");
const eyes = ref("0.2");
// 眼睛提亮（FEAT-210）：0~100 的百分数滑条，默认 10%；提交时换算成 0~1
const eyeBright = ref(10);
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

const job = computed(() => store.jobOfTool("face_reshape"));
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
      tool: "face_reshape",
      paths: props.paths,
      prompt: "",
      outputDir: outputDir.value,
      options: {
        slim: Number(slim.value) || 0,
        eyes: Number(eyes.value) || 0,
        eye_bright: (Number(eyeBright.value) || 0) / 100,
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
  const id = store.activeJobId("face_reshape");
  if (id) await store.cancel(id);
}
</script>

<template>
  <section class="ai-tool">
    <div class="ai-panel">
      <div class="ai-field">
        <label class="ai-label">瘦脸强度</label>
        <div class="ai-seg">
          <button
            v-for="s in RESHAPE_SLIM_LEVELS"
            :key="s.value"
            type="button"
            class="ai-seg-btn"
            :class="{ on: slim === s.value }"
            :title="s.hint"
            :disabled="busy"
            @click="slim = s.value"
          >
            {{ s.label }}
          </button>
        </div>
      </div>

      <div class="ai-field">
        <label class="ai-label">大眼强度</label>
        <div class="ai-seg">
          <button
            v-for="s in RESHAPE_EYE_LEVELS"
            :key="s.value"
            type="button"
            class="ai-seg-btn"
            :class="{ on: eyes === s.value }"
            :title="s.hint"
            :disabled="busy"
            @click="eyes = s.value"
          >
            {{ s.label }}
          </button>
        </div>
      </div>

      <div class="ai-field">
        <label class="ai-label">眼睛提亮 {{ eyeBright }}%</label>
        <input v-model.number="eyeBright" type="range" min="0" max="100" step="5" :disabled="busy" />
        <p class="ai-note" style="margin: 4px 0 0">
          只提亮眼白（低彩度区），虹膜与眼线基本不动；调到 0 = 不处理。
        </p>
      </div>

      <p class="ai-note" style="margin: 0">
        只收下颌、放大眼睛并提亮眼神，五官位置与比例不变；侧脸、遮挡或关键点置信度低时会自动跳过该脸，不会硬修。
        这里按你选的强度执行（配方/工作台自动档才会按发长收敛）。
      </p>
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <LocalModelHint :ids="['face_landmark_68']" />
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <OutputDirField :output-dir="outputDir" :disabled="busy" @update:output-dir="onDir" />
    </div>

    <p class="ai-note ai-note-block">
      全程本机完成，照片不上传、不花钱；输出为新 JPG，分辨率不变，原图不动。
      未检出人脸的照片会原样输出并提示。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        开始形变（{{ paths.length }} 张）
      </button>
    </div>

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>
