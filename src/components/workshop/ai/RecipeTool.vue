<script setup lang="ts">
/**
 * ① 一键出片（FEAT-168 默认落地页）
 *
 * 从导入到出图只有三步：选图（共用托盘）→ 选配方 → 出片。
 * 全程不出现任何单算子、不出现调参面板（产品定位：懒人快速出图，不做精修）。
 *
 * 作业骨架复用 useToolJob，配方选择复用 RecipePicker（与 ②批量套用同一个组件），
 * 步骤进度由 JobStepsPanel 只读展示。本组件只编排这三者，不含图像处理逻辑。
 */
import { computed, ref, toRef } from "vue";
import { useToolJob } from "../../../composables/useToolJob";
import OutputDirField from "./OutputDirField.vue";
import AiResultPanel from "./AiResultPanel.vue";
import RecipePicker from "./RecipePicker.vue";
import JobStepsPanel from "./JobStepsPanel.vue";

const props = defineProps<{ paths: string[] }>();
const recipeId = ref("natural");
const paths = toRef(props, "paths");

const { outputDir, job, busy, canRun, run, cancel, onDir } = useToolJob({
  tool: "recipe",
  paths,
  options: () => ({ recipeId: recipeId.value }),
});

const runLabel = computed(() => `一键出片（${props.paths.length} 张）`);
</script>

<template>
  <section class="ai-tool">
    <RecipePicker v-model="recipeId" :disabled="busy" />

    <div class="ai-panel">
      <OutputDirField :output-dir="outputDir" :disabled="busy" @update:output-dir="onDir" />
    </div>

    <p class="ai-note ai-note-block">
      只处理 LR 导出的 JPG/PNG：调色、滤镜、人像与高清按配方一次跑完，原图不动。
      缺模型的可选步骤会自动跳过并提示，不会让整批失败。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ runLabel }}
      </button>
    </div>

    <JobStepsPanel :job="job" />
    <AiResultPanel :job="job" @cancel="cancel" />
  </section>
</template>
