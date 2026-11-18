<script setup lang="ts">
/**
 * 胶片模拟（FEAT-163）
 * 内置多款胶片 LUT，可选强度 / 颗粒 / 暗角，纯本机推理，输出 JPG 副本。
 */
import { computed, onMounted, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { dirName, joinPath } from "../../../utils/pathParts";
import type { LocalLutEntry } from "../../../types/ai";
import OutputDirField from "./OutputDirField.vue";
import AiResultPanel from "./AiResultPanel.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

const luts = ref<LocalLutEntry[]>([]);
const lut = ref("provia");
const strength = ref(100);
const grain = ref(0);
const vignette = ref(0);
const outputDir = ref("");
const outputTouched = ref(false);

onMounted(async () => {
  try {
    const st = await store.localModels();
    luts.value = st.luts;
    if (st.luts.length && !st.luts.some((l) => l.id === lut.value)) lut.value = st.luts[0].id;
  } catch {
    /* 状态拿不到时不阻塞：后端仍会按 id 校验 */
  }
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

const job = computed(() => store.jobOfTool("film_sim"));
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
      tool: "film_sim",
      paths: props.paths,
      prompt: "",
      outputDir: outputDir.value,
      options: {
        lut: lut.value,
        strength: strength.value / 100,
        grain: grain.value / 100,
        vignette: vignette.value / 100,
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
  const id = store.activeJobId("film_sim");
  if (id) await store.cancel(id);
}
</script>

<template>
  <section class="ai-tool">
    <div class="ai-panel">
      <div class="ai-field">
        <label class="ai-label">胶片风格</label>
        <div class="ai-row">
          <select v-model="lut" class="ai-sel" :disabled="busy">
            <option v-for="l in luts" :key="l.id" :value="l.id">{{ l.name }}</option>
            <option v-if="!luts.length" value="provia">Provia 标准</option>
          </select>
        </div>
      </div>

      <div class="ai-field">
        <label class="ai-label">滤镜强度 {{ strength }}%</label>
        <input v-model.number="strength" type="range" min="0" max="100" :disabled="busy" />
      </div>
      <div class="ai-field">
        <label class="ai-label">胶片颗粒 {{ grain }}%</label>
        <input v-model.number="grain" type="range" min="0" max="100" :disabled="busy" />
      </div>
      <div class="ai-field">
        <label class="ai-label">暗角 {{ vignette }}%</label>
        <input v-model.number="vignette" type="range" min="0" max="100" :disabled="busy" />
      </div>
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <OutputDirField :output-dir="outputDir" :disabled="busy" @update:output-dir="onDir" />
    </div>

    <p class="ai-note">
      内置富士系胶片 LUT（随包提供，无需下载），全程在本机完成，照片不上传；输出为新 JPG，不覆盖原图。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "处理中…" : `开始模拟（${paths.length} 张）` }}
      </button>
    </div>

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>
