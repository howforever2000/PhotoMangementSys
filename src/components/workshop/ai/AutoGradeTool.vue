<script setup lang="ts">
/**
 * 智能调色（FEAT-165）
 * 按画面统计自动推导调色参数，本机查表出图，可导出 .cube 复用。
 * 纯本机、零模型、零费用、保分辨率（不做任何重绘）。
 */
import { computed, onMounted, ref, toRef, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { useRememberedParams } from "../../../composables/useRememberedParams";
import { dirName, joinPath } from "../../../utils/pathParts";
import { AUTOGRADE_STYLES } from "../../../utils/aiPresets";
import OutputDirField from "./OutputDirField.vue";
import AiResultPanel from "./AiResultPanel.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

const style = ref("natural");
/** 力度：跨会话记住（调好的力度下次重进还在）；风格档不记（见 toolParamsMemory 白名单） */
const { params: remembered, resetToDefault } = useRememberedParams(
  "auto_grade",
  { strength: 100 },
  { clamp: { strength: [0, 100] } },
);
const strength = toRef(remembered, "strength");
/** 自然饱和度滑条已移除（FEAT-207）：内嵌 vibrance（0=不做）由独立的
 *  `local_vibrance` 算子/配方末步替代（双向、中性 50），调色算子自身
 *  只按风格派生值走。 */
const exportCube = ref(false);
const outputDir = ref("");
const outputTouched = ref(false);

onMounted(() => {
  /* 参数全在本机，无需预取远端状态 */
});

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

const job = computed(() => store.jobOfTool("auto_grade"));
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
      tool: "auto_grade",
      paths: props.paths,
      prompt: "",
      outputDir: outputDir.value,
      options: {
        style: style.value,
        strength: strength.value / 100,
        exportCube: exportCube.value,
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
  const id = store.activeJobId("auto_grade");
  if (id) await store.cancel(id);
}
</script>

<template>
  <section class="ai-tool">
    <div class="ai-panel">
      <div class="ai-field">
        <label class="ai-label">调色风格</label>
        <div class="ai-seg">
          <button
            v-for="s in AUTOGRADE_STYLES"
            :key="s.value"
            type="button"
            class="ai-seg-btn"
            :class="{ on: style === s.value }"
            :disabled="busy"
            @click="style = s.value"
          >
            {{ s.label }}
          </button>
        </div>
      </div>

      <div class="ai-field">
        <label class="ai-label">力度 {{ strength }}%</label>
        <input v-model.number="strength" type="range" min="0" max="100" :disabled="busy" />
      </div>

      <div class="ai-field">
        <label class="ai-row" style="gap: 6px">
          <input v-model="exportCube" type="checkbox" :disabled="busy" />
          <span class="ai-label" style="margin: 0">同时导出 .cube（可当滤镜复用）</span>
        </label>
      </div>
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <OutputDirField :output-dir="outputDir" :disabled="busy" @update:output-dir="onDir" />
    </div>

    <p class="ai-note ai-note-block">
      逐张分析画面的明暗、色偏与饱和度后自动调色，不重绘像素、不改变分辨率，
      全程本机完成；输出为新 JPG，不覆盖原图。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        开始调色（{{ paths.length }} 张）
      </button>
      <button
        class="ai-ghost"
        type="button"
        :disabled="busy"
        title="只把本工具的参数恢复成默认值，不影响其它工具"
        @click="resetToDefault"
      >
        恢复默认参数
      </button>
    </div>

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>
