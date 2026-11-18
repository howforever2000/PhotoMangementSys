<script setup lang="ts">
/** 输出目录选择（FEAT-163）：本地增强三工具共用。 */
import { open as openDialog } from "@tauri-apps/plugin-dialog";

const props = defineProps<{ outputDir: string; disabled?: boolean }>();
const emit = defineEmits<{ "update:outputDir": [value: string] }>();

async function pick() {
  const dir = await openDialog({
    directory: true,
    title: "选择输出目录",
    defaultPath: props.outputDir || undefined,
  });
  if (typeof dir === "string") emit("update:outputDir", dir);
}
</script>

<template>
  <div class="ai-field">
    <label class="ai-label">输出到</label>
    <div class="ai-row">
      <span class="ai-ref-path" :title="outputDir || '尚未选择'">{{ outputDir || "尚未选择目录" }}</span>
      <button class="ai-ghost" type="button" :disabled="disabled" @click="pick">选择…</button>
    </div>
  </div>
</template>
