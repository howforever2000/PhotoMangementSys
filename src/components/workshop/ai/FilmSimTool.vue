<script setup lang="ts">
/**
 * 胶片模拟（FEAT-163）
 * 两条引擎：内置 .cube LUT（默认，老行为不变）与参数化自适应渲染（新增 2 款风格只有这条）。
 * 强度分两条线：look（轻/标准/浓）动「偏离原厂」的程度，strength 动「成片与原图」的混合比例。
 */
import { computed, onMounted, ref, toRef, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { useRememberedParams } from "../../../composables/useRememberedParams";
import { dirName, joinPath } from "../../../utils/pathParts";
import type { LocalLutEntry } from "../../../types/ai";
import OutputDirField from "./OutputDirField.vue";
import AiResultPanel from "./AiResultPanel.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

const luts = ref<LocalLutEntry[]>([]);
/**
 * 参数跨会话记住：两条强度线 + 颗粒/暗角 + 胶片档与调色档。
 * `lut` 的合法性由下面 onMounted 的 LUT 列表兜底（记的档案已不存在就回落第一款）。
 */
const { params: remembered, resetToDefault } = useRememberedParams(
  "film_sim",
  { lut: "provia", strength: 100, look: 100, grade: "", gradeStrength: 100, grain: 0, vignette: 0 },
  {
    clamp: {
      strength: [0, 100],
      look: [60, 140],
      gradeStrength: [0, 140],
      grain: [0, 100],
      vignette: [0, 100],
    },
  },
);
const lut = toRef(remembered, "lut");
const strength = toRef(remembered, "strength");
const look = toRef(remembered, "look");
const grade = toRef(remembered, "grade");
const gradeStrength = toRef(remembered, "gradeStrength");
const grain = toRef(remembered, "grain");
const vignette = toRef(remembered, "vignette");
const outputDir = ref("");
const outputTouched = ref(false);

const selected = computed(() => luts.value.find((l) => l.id === lut.value));
const isFilm = computed(() => selected.value?.engine === "film");

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
        engine: selected.value?.engine ?? "auto",
        strength: strength.value / 100,
        look: look.value / 100,
        grade: isFilm.value ? grade.value : "",
        grade_strength: gradeStrength.value / 100,
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
        <p v-if="selected?.scene" class="ai-note" style="margin-top: 6px">
          {{ selected.scene }}<template v-if="selected.why"> · {{ selected.why }}</template>
        </p>
      </div>

      <div class="ai-field">
        <label class="ai-label">滤镜强度 {{ strength }}%</label>
        <input v-model.number="strength" type="range" min="0" max="100" :disabled="busy" />
      </div>

      <template v-if="isFilm">
        <div class="ai-field">
          <label class="ai-label">风格强度（轻 / 标准 / 浓）{{ look }}%</label>
          <input v-model.number="look" type="range" min="60" max="140" :disabled="busy" />
        </div>
        <div class="ai-field">
          <label class="ai-label">后调（偏色层）</label>
          <div class="ai-row">
            <select v-model="grade" class="ai-sel" :disabled="busy">
              <option value="">不叠加</option>
              <option value="teal_orange">青橙</option>
              <option value="vintage_amber">复古琥珀</option>
              <option value="cold_cyan">冷青灰</option>
            </select>
          </div>
        </div>
        <div v-if="grade" class="ai-field">
          <label class="ai-label">后调强度 {{ gradeStrength }}%</label>
          <input v-model.number="gradeStrength" type="range" min="0" max="140" :disabled="busy" />
        </div>
      </template>

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

    <p class="ai-note ai-note-block">
      内置富士系 LUT（老路径，逐位不变）与参数化自适应引擎（按场景统计渲染，高光截断 ≤0.5%）。
      新增「人像 Portra 400 / 电影 2383」只有参数化版；全程本机，照片不上传，输出为新 JPG。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "处理中…" : `开始模拟（${paths.length} 张）` }}
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
