<script setup lang="ts">
/**
 * 参考图调色（FEAT-163；多参考图共性 FEAT-211；三层解耦 FEAT-219；参数收敛 FEAT-220）
 * 把参考图的色调 / 影调迁到素材图。算法只保留「智能分层」（影调/色偏/同色三层解耦），
 * 旧「经典统计」「强匹配」仅继续服务已保存的链条步骤，这里不再暴露入口；
 * 用户侧只留「强度」一个滑条 + 「保护肤色」开关。
 * 支持多张参考图聚类提取共性；可选把这次迁移导出成 .cube（按首张素材冻结）。
 */
import { computed, ref, toRef, watch } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { useRememberedParams } from "../../../composables/useRememberedParams";
import { baseName, dirName, joinPath } from "../../../utils/pathParts";
import OutputDirField from "./OutputDirField.vue";
import AiResultPanel from "./AiResultPanel.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

/** 强度：跨会话记住；保护肤色开关按「肤色不记」的口径排除（见 toolParamsMemory 白名单） */
const { params: remembered, resetToDefault } = useRememberedParams(
  "color_transfer",
  { strength: 100 },
  { clamp: { strength: [0, 100] } },
);
const strength = toRef(remembered, "strength");
const protectSkin = ref(false);
const exportCube = ref(false);
const referencePaths = ref<string[]>([]);
const outputDir = ref("");
const outputTouched = ref(false);

const MAX_REFS = 10;

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

const job = computed(() => store.jobOfTool("color_transfer"));
const busy = computed(
  () => !!job.value && (job.value.status === "queued" || job.value.status === "running"),
);
const canRun = computed(() => props.paths.length > 0 && !busy.value);

async function pickRef() {
  const picked = await openDialog({
    multiple: true,
    directory: false,
    title: "选择参考图（可多选，提取共性色调）",
    filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp", "bmp"] }],
  });
  if (!picked) return;
  const list = Array.isArray(picked) ? picked : [picked];
  const merged = [...referencePaths.value];
  let overflow = 0;
  for (const p of list) {
    if (merged.includes(p)) continue;
    if (merged.length >= MAX_REFS) {
      overflow += 1;
      continue;
    }
    merged.push(p);
  }
  referencePaths.value = merged;
  if (overflow > 0) {
    notify.warning("参考图过多", `最多 ${MAX_REFS} 张，多余的 ${overflow} 张没有加入`);
  }
}

function removeRef(p: string) {
  referencePaths.value = referencePaths.value.filter((x) => x !== p);
}

async function run() {
  if (!outputDir.value) {
    notify.warning("还没有输出目录", "先选择结果要放在哪个文件夹");
    return;
  }
  if (!referencePaths.value.length) {
    notify.warning("缺少参考图", "参考图调色至少需要选择一张参考图");
    return;
  }
  try {
    await store.run({
      tool: "color_transfer",
      paths: props.paths,
      prompt: "",
      outputDir: outputDir.value,
      options: {
        referencePaths: referencePaths.value,
        method: "adaptive",
        strength: strength.value / 100,
        protectSkin: protectSkin.value,
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
  const id = store.activeJobId("color_transfer");
  if (id) await store.cancel(id);
}
</script>

<template>
  <section class="ai-tool">
    <div class="ai-panel">
      <div class="ai-field">
        <label class="ai-label">参考图{{ referencePaths.length > 1 ? `（${referencePaths.length} 张，聚类提取共性）` : "" }}</label>
        <div v-if="referencePaths.length" class="ai-ref-list">
          <span
            v-for="p in referencePaths"
            :key="p"
            class="ai-ref-chip"
            :title="p"
          >
            {{ baseName(p) }}
            <button type="button" class="ai-ref-x" :disabled="busy" @click="removeRef(p)">×</button>
          </span>
        </div>
        <div class="ai-row">
          <span v-if="!referencePaths.length" class="ai-ref-path">尚未选择参考图</span>
          <button class="ai-ghost" type="button" :disabled="busy" @click="pickRef">
            {{ referencePaths.length ? "添加参考图…" : "选择参考图…" }}
          </button>
        </div>
      </div>

      <div class="ai-field">
        <label class="ai-label">迁移强度 {{ strength }}%</label>
        <input v-model.number="strength" type="range" min="0" max="100" :disabled="busy" />
      </div>

      <label class="ai-row" style="gap: 6px; cursor: pointer">
        <input v-model="protectSkin" type="checkbox" :disabled="busy" />
        <span class="ai-label" style="letter-spacing: normal">保护肤色（人像的色相少被整体色调带走）</span>
      </label>

      <label class="ai-row" style="gap: 6px; cursor: pointer">
        <input v-model="exportCube" type="checkbox" :disabled="busy" />
        <span class="ai-label" style="letter-spacing: normal">
          同时导出一份 .cube 滤镜（按第一张素材冻结）
        </span>
      </label>
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <OutputDirField :output-dir="outputDir" :disabled="busy" @update:output-dir="onDir" />
    </div>

    <p class="ai-note ai-note-block">
      智能分层：把影调、整体色偏、同色换色分开迁移，100% 强度也能保持曝光与高光。参考图里没有的颜色会保持原样（不会硬套成参考色）。全程本机完成、不上传。输出为新 JPG；勾选导出后会额外生成一个 .cube 滤镜文件。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "处理中…" : `开始调色（${paths.length} 张）` }}
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
