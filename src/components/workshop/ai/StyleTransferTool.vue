<script setup lang="ts">
/**
 * 图像滤镜迁移 / 自主调色工具（FEAT-152）
 * - 参考图迁移：输入内容图（素材托盘）+ 参考图，迁移色调/影调
 * - 自主调色：仅凭提示词对素材图调色
 */
import { computed, ref, watch } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { baseName, dirName, joinPath } from "../../../utils/pathParts";
import { STYLE_MODES } from "../../../utils/aiPresets";
import AiPromptPanel from "./AiPromptPanel.vue";
import AiResultPanel from "./AiResultPanel.vue";
import AiCostHint from "./AiCostHint.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

const mode = ref("transfer");
const referencePath = ref("");
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
const job = computed(() => store.jobOfTool("style_transfer"));
const busy = computed(
  () => !!job.value && (job.value.status === "queued" || job.value.status === "running"),
);
const canRun = computed(() => props.paths.length > 0 && !busy.value);

async function pickRef() {
  const picked = await openDialog({
    multiple: false,
    directory: false,
    title: "选择参考图（滤镜 / 调色来源）",
    filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp", "bmp"] }],
  });
  if (typeof picked === "string") referencePath.value = picked;
}

async function run() {
  if (!outputDir.value) {
    notify.warning("还没有输出目录", "先选择结果要放在哪个文件夹");
    return;
  }
  if (mode.value === "transfer" && !referencePath.value) {
    notify.warning("缺少参考图", "参考图迁移需要选择一张参考图");
    return;
  }
  try {
    await store.run({
      tool: "style_transfer",
      paths: props.paths,
      prompt: prompt.value,
      outputDir: outputDir.value,
      options: { mode: mode.value, referencePath: referencePath.value },
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
  const id = store.activeJobId("style_transfer");
  if (id) await store.cancel(id);
}
</script>

<template>
  <section class="ai-tool">
    <div class="ai-panel">
      <div class="ai-field">
        <label class="ai-label">模式</label>
        <div class="ai-seg">
          <button
            v-for="m in STYLE_MODES"
            :key="m.value"
            type="button"
            class="ai-seg-btn"
            :class="{ on: mode === m.value }"
            :title="m.hint"
            @click="mode = m.value"
          >
            {{ m.label }}
          </button>
        </div>
      </div>

      <div v-if="mode === 'transfer'" class="ai-field">
        <label class="ai-label">参考图</label>
        <div class="ai-row">
          <span class="ai-ref-path" :title="referencePath || '尚未选择'">
            {{ referencePath ? baseName(referencePath) : "尚未选择参考图" }}
          </span>
          <button class="ai-ghost" type="button" :disabled="busy" @click="pickRef">选择参考图…</button>
        </div>
      </div>
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <AiPromptPanel
        :prompt="prompt"
        :output-dir="outputDir"
        :disabled="busy"
        :placeholder="mode === 'transfer' ? '补充要求（可选），例如：只迁移色调，不改构图…' : '描述想要的调色方向，例如：电影感青橙、低饱和暖调…'"
        @update:prompt="prompt = $event"
        @update:outputDir="onDir"
      />
    </div>

    <p class="ai-note">
      参考图迁移会把参考图的色调与影调迁移到素材图；自主调色仅凭提示词调色。两者都保留原构图，输出为新 JPG。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "处理中…" : `开始处理（${paths.length} 张）` }}
      </button>
    </div>

    <AiCostHint tool="style_transfer" :count="paths.length" />

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>
