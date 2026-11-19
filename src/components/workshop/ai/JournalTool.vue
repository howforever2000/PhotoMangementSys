<script setup lang="ts">
/**
 * 主题手账工具（FEAT-152）
 * 版式模板 × 画面风格 + 提示词 → 生成手账（1 张 JPG / 多张 PDF）。
 *
 * FEAT-170：画面风格按 `group` 分组呈现（绘画 / 卡通形象）。分组只是排版 ——
 * 提交给后端的仍只有 style key，后端不按组做任何分支。
 */
import { computed, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { dirName, joinPath } from "../../../utils/pathParts";
import { JOURNAL_STYLES, JOURNAL_STYLE_GROUPS, JOURNAL_TEMPLATES } from "../../../utils/aiPresets";
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

// 作业归属提在 store（activeByTool）：切走页面/工具再回来能按工具名取回作业重连
const job = computed(() => store.jobOfTool("journal"));

/** 按分组切好的风格选项：空组不渲染（后端新增了组、前端还没登记时不会出现空标题） */
const styleGroups = computed(() =>
  JOURNAL_STYLE_GROUPS.map((g) => ({
    ...g,
    items: JOURNAL_STYLES.filter((s) => s.group === g.value),
  })).filter((g) => g.items.length > 0),
);
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
    await store.run({
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
  const id = store.activeJobId("journal");
  if (id) await store.cancel(id);
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
        <div v-for="g in styleGroups" :key="g.value" class="ai-style-group">
          <span class="ai-style-group-label">{{ g.label }}</span>
          <div class="ai-seg">
            <button
              v-for="s in g.items"
              :key="s.value"
              type="button"
              class="ai-seg-btn"
              :class="{ on: style === s.value }"
              :title="s.hint"
              @click="style = s.value"
            >
              {{ s.label }}
            </button>
          </div>
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

    <AiCostHint tool="journal" :count="paths.length" :settings="store.settings" />

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>
