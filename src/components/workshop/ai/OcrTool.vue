<script setup lang="ts">
/**
 * 文字提取（FEAT-171）：本机离线 OCR —— det 找框 → rec 认字 → 出可复制文本。
 *
 * 模型分区（det 扫描区共享 / rec 工坊区下载）见 ocr_engine.py 头注：
 *   - det（检测）：扫描区共享件，模型管理「文字 · OCR 检测」下载；
 *   - rec（识别）：工坊区下载入口在本面板的 LocalModelHint（Python id `paddleocr_rec`）。
 * 字典 ppocr_keys_v1.txt 随包内置，用户无感。
 *
 * 产物：`<输出目录>/<原名>_文字.txt`；识别出的文本同时回显在下方，可一键复制。
 * 就绪状态单独轮询 —— LocalModelHint 是下载入口，不负责就绪文案。
 */
import { computed, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useContentStore } from "../../../stores/content";
import { useNotify } from "../../../composables/useNotify";
import { dirName, joinPath } from "../../../utils/pathParts";
import OutputDirField from "./OutputDirField.vue";
import LocalModelHint from "./LocalModelHint.vue";
import AiResultPanel from "./AiResultPanel.vue";

const props = defineProps<{ paths: string[] }>();
const store = useAiStudioStore();
const content = useContentStore();
const notify = useNotify();

const outputDir = ref("");
const outputTouched = ref(false);
/** 最近一次完成作业的识别全文（job.meta.text） */
const lastText = ref("");

watch(
  () => props.paths,
  (paths) => {
    if (outputTouched.value) return;
    const first = paths[0];
    if (!first) return;
    const dir = dirName(first);
    outputDir.value = dir ? joinPath(dir, "_ai") : "";
  },
  { immediate: true },
);

const job = computed(() => store.jobOfTool("ocr"));
const busy = computed(
  () => !!job.value && (job.value.status === "queued" || job.value.status === "running"),
);

/** rec 下载状态：与 LocalModelHint 同一数据源（list_model_downloads，事件实时刷新） */
const recDone = computed(() => {
  const row = content.modelDownloads.find((d) => d.name === "local-ocr-rec");
  return !!row?.done;
});
const canRun = computed(() => props.paths.length > 0 && !busy.value && recDone.value);

/** 作业完成时把 meta.text 抓出来回显（store 不透传 meta 类型，就地取） */
watch(job, (j) => {
  if (j?.status === "done") {
    const meta = (j as unknown as { meta?: { text?: string } }).meta;
    lastText.value = meta?.text ?? "";
  }
});

async function run() {
  if (!outputDir.value) {
    notify.warning("还没有输出目录", "先选择结果要放在哪个文件夹");
    return;
  }
  if (!recDone.value) {
    notify.warning("识别模型未就绪", "先在下方下载「文字提取 · OCR 识别」");
    return;
  }
  try {
    await store.run({
      tool: "ocr",
      paths: props.paths,
      prompt: "",
      outputDir: outputDir.value,
      options: {},
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
  const id = store.activeJobId("ocr");
  if (id) await store.cancel(id);
}

/** 复制识别全文：clipboard API 优先，非安全上下文回落 textarea execCommand（先例 ImportErrorsDialog） */
async function copyText() {
  const text = lastText.value;
  if (!text) return;
  let ok = false;
  if (navigator.clipboard && window.isSecureContext) {
    try {
      await navigator.clipboard.writeText(text);
      ok = true;
    } catch {
      ok = false;
    }
  }
  if (!ok) {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    try {
      ok = document.execCommand("copy");
    } catch {
      ok = false;
    }
    document.body.removeChild(ta);
  }
  if (ok) {
    notify.success("已复制", "识别文本已复制到剪贴板");
  } else {
    notify.error("复制失败", "请打开文本文件手动复制");
  }
}
</script>

<template>
  <section class="ai-tool">
    <div class="ai-panel">
      <div class="ai-field">
        <label class="ai-label">说明</label>
        <div class="ocr-ready">
          <span class="ocr-status" :class="{ bad: !recDone }">
            {{
              recDone
                ? "✓ 识别模型已就绪；检测模型随扫描区共享（缺了会提示去模型管理下载）"
                : "识别模型未就绪：在下方下载后立即可用"
            }}
          </span>
        </div>
      </div>

      <LocalModelHint />
    </div>

    <div class="ai-panel" style="margin-top: 12px">
      <OutputDirField :output-dir="outputDir" :disabled="busy" @update:output-dir="onDir" />
    </div>

    <p class="ai-note">
      全程在本机离线完成，照片不上传；识别结果按阅读顺序写入
      <code>_文字.txt</code>，同时回显在结果区可一键复制。
    </p>

    <div class="ai-actions">
      <button class="ai-run" type="button" :disabled="!canRun" @click="run">
        {{ busy ? "识别中…" : `开始提取（${paths.length} 张）` }}
      </button>
      <span v-if="!recDone" class="ai-note" style="margin: 0 0 0 10px">
        {{ busy ? "" : "先下载识别模型后开始" }}
      </span>
    </div>

    <AiResultPanel :job="job" @cancel="onCancel" />

    <div v-if="lastText" class="ai-panel" style="margin-top: 12px">
      <div class="ai-row" style="justify-content: space-between; margin-bottom: 6px">
        <label class="ai-label">识别全文</label>
        <span style="display: flex; gap: 8px">
          <button class="ai-ghost" type="button" @click="copyText">复制全文</button>
          <button class="ai-ghost" type="button" @click="lastText = ''">收起</button>
        </span>
      </div>
      <pre class="ocr-text">{{ lastText }}</pre>
    </div>
  </section>
</template>

<style scoped>
.ocr-ready {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.ocr-status {
  font-size: var(--fs-meta);
}
.ocr-status.ok {
  color: var(--fg-2);
}
.ocr-status.bad {
  color: #c0392b;
}
.ocr-text {
  max-height: 260px;
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-word;
  font-size: var(--fs-meta);
  line-height: 1.5;
  padding: 8px 10px;
  border-radius: var(--r-panel, 8px);
  background: var(--hover, rgba(0, 0, 0, 0.04));
  margin: 0;
}
</style>
