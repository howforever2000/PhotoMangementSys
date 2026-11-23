<script setup lang="ts">
/**
 * 参考图调色（FEAT-163；多参考图共性 FEAT-211）
 * 把参考图的色调 / 影调迁到素材图（Reinhard 自然 / 直方图强匹配），纯本机；
 * 支持选多张参考图等权提取共性色调；可选把这次迁移导出成 .cube 供以后当滤镜复用。
 */
import { computed, ref, watch } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { baseName, dirName, joinPath } from "../../../utils/pathParts";
import { CT_METHODS } from "../../../utils/aiPresets";
import OutputDirField from "./OutputDirField.vue";
import AiResultPanel from "./AiResultPanel.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const notify = useNotify();

const method = ref("reinhard");
const strength = ref(80);
const lWeight = ref(100);
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
        method: method.value,
        strength: strength.value / 100,
        lWeight: lWeight.value / 100,
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
        <label class="ai-label">参考图{{ referencePaths.length > 1 ? `（${referencePaths.length} 张，等权提取共性）` : "" }}</label>
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
        <label class="ai-label">迁移方式</label>
        <div class="ai-seg">
          <button
            v-for="m in CT_METHODS"
            :key="m.value"
            type="button"
            class="ai-seg-btn"
            :class="{ on: method === m.value }"
            :title="m.hint"
            @click="method = m.value"
          >
            {{ m.label }}
          </button>
        </div>
      </div>

      <div class="ai-field">
        <label class="ai-label">迁移强度 {{ strength }}%</label>
        <input v-model.number="strength" type="range" min="0" max="100" :disabled="busy" />
      </div>

      <div class="ai-field">
        <label class="ai-label">明度迁移 {{ lWeight }}%</label>
        <input v-model.number="lWeight" type="range" min="0" max="100" :disabled="busy" />
      </div>

      <label class="ai-row" style="gap: 6px; cursor: pointer">
        <input v-model="exportCube" type="checkbox" :disabled="busy" />
        <span class="ai-label" style="letter-spacing: normal">同时导出一份 .cube 滤镜</span>
      </label>
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <OutputDirField :output-dir="outputDir" :disabled="busy" @update:output-dir="onDir" />
    </div>

    <p class="ai-note ai-note-block">
      把参考图的色调与影调迁移到素材图，保留原构图；全程本机完成、不上传。输出为新 JPG；勾选导出后会额外生成一个 .cube 滤镜文件。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "处理中…" : `开始调色（${paths.length} 张）` }}
      </button>
    </div>

    <AiResultPanel :job="job" @cancel="onCancel" />
  </section>
</template>
