<script setup lang="ts">
/**
 * 滤镜迁移（FEAT-152 → FEAT-171）
 *
 * 参考图来源：从参考图提取影调描述后迁移；不选参考图时按提示词自动调色
 * （mode=autograde）。原「风格预设」云端来源已随 FEAT-171 移除，本机预设链路
 * 也已下线 —— 想要固定影调就选一张参考图。
 *
 * ## 形状纪律（FEAT-169）
 *
 * 后端提示词一律追加 `presets.STRUCTURE_LOCK` 硬段，出图后再经
 * `critic.structure_compare` 校验，检出结构漂移会在结果里告警。
 * 界面因此要提前把「要绝对不变就用本机」讲清楚：本机是逐色查表，
 * 几何恒等是数学保证；云端是整幅重绘，只能靠约束 + 校验逼近。
 */
import { computed, ref, watch } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { baseName, dirName, joinPath } from "../../../utils/pathParts";
import { STYLE_MODES } from "../../../utils/aiPresets";
import { STRUCTURE_LOCK_ZH } from "../../../utils/structureLock";
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

/** transfer 模式必须有参考图 */
const needSource = computed(() => mode.value === "transfer" && !referencePath.value);
const canRun = computed(() => props.paths.length > 0 && !busy.value && !needSource.value);

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
  if (needSource.value) {
    notify.warning("缺少迁移来源", "先指定一张参考图，或切换到「自动调色」模式");
    return;
  }
  try {
    await store.run({
      tool: "style_transfer",
      paths: props.paths,
      prompt: prompt.value,
      outputDir: outputDir.value,
      options: {
        mode: mode.value,
        referencePath: referencePath.value,
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

      <div class="ai-field">
        <label class="ai-label">参考图{{ mode === "transfer" ? "" : "（可选）" }}</label>
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
      参考图只改变色调与影调；<strong class="lock">{{ STRUCTURE_LOCK_ZH }}</strong>
      模型每张出图后会做结构校验，检出脸型或构图被改会告警。要绝对保证形状不变，
      请用「影调与滤镜」里的本机工具——那条链路是逐色查表，几何恒等。
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

<style scoped>
.lock {
  font-weight: 600;
  color: var(--fg);
}
</style>
