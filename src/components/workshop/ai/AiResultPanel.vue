<script setup lang="ts">
/**
 * AI 作业结果面板（FEAT-152）：进度条 + 阶段 + 产物列表 + 警告/错误。
 * 四个工具共用；只做展示与「打开所在目录」，不持有业务状态。
 */
import { computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { AiJobStatus } from "../../../types/ai";
import { baseName, dirName } from "../../../utils/pathParts";

const props = defineProps<{ job: AiJobStatus | null }>();
const emit = defineEmits<{ cancel: [] }>();

const busy = computed(
  () => !!props.job && (props.job.status === "queued" || props.job.status === "running"),
);

const statusText = computed(() => {
  const j = props.job;
  if (!j) return "";
  switch (j.status) {
    case "queued":
      return "排队中";
    case "running":
      return `${j.stage || "处理中"} ${j.progress}%`;
    case "done":
      return "完成";
    case "failed":
      return "失败";
    case "cancelled":
      return "已取消";
    default:
      return j.status;
  }
});

function badge(path: string): string {
  const ext = path.split(".").pop() || "";
  return ext.toUpperCase();
}

async function openDir(path: string) {
  try {
    await invoke("open_folder", { path: dirName(path) });
  } catch {
    /* 打不开就静默（不影响结果展示） */
  }
}
</script>

<template>
  <section v-if="job" class="ai-result">
    <div class="ai-result-head">
      <strong>{{ statusText }}</strong>
      <span v-if="job.message" class="ai-dim">{{ job.message }}</span>
      <span class="ai-dim" style="margin-left: auto">{{ job.outputs.length }} 个产物</span>
      <button v-if="busy" class="ai-ghost" type="button" @click="emit('cancel')">停止</button>
    </div>

    <div v-if="busy" class="ai-bar"><i :style="{ width: job.progress + '%' }" /></div>

    <div v-for="p in job.outputs" :key="p" class="ai-out-row">
      <span class="ai-out-badge">{{ badge(p) }}</span>
      <span class="ai-out-name" :title="p">{{ baseName(p) }}</span>
      <button class="ai-ghost" type="button" @click="openDir(p)">打开所在目录</button>
    </div>

    <div v-if="job.error" class="ai-out-row">
      <span class="ai-warn">{{ job.error }}</span>
    </div>
    <div v-for="(w, i) in job.warnings" :key="i" class="ai-out-row">
      <span class="ai-warn">{{ w }}</span>
    </div>
  </section>
</template>
