<script setup lang="ts">
/**
 * AI 通用输入面板（FEAT-152）：提示词 + 输出目录 + 隐私告知。
 * 四个 AI 工具共用，避免各写一份「提示词框 + 选目录」。
 */
import { open as openDialog } from "@tauri-apps/plugin-dialog";

const props = defineProps<{
  prompt: string;
  outputDir: string;
  disabled?: boolean;
  placeholder?: string;
  rows?: number;
  /** 是否显示隐私告知（默认显示；内部工具可关） */
  privacy?: boolean;
}>();

const emit = defineEmits<{
  "update:prompt": [value: string];
  "update:outputDir": [value: string];
}>();

async function pickDir() {
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
    <label class="ai-label">提示词（可选）</label>
    <textarea
      class="ai-ta"
      :value="prompt"
      :rows="rows || 3"
      :disabled="disabled"
      :placeholder="placeholder || '用一句话描述你想要的效果…'"
      @input="emit('update:prompt', ($event.target as HTMLTextAreaElement).value)"
    />
  </div>

  <div class="ai-field">
    <label class="ai-label">输出到</label>
    <div class="ai-row">
      <span class="ai-ref-path" :title="outputDir || '尚未选择'">{{ outputDir || "尚未选择目录" }}</span>
      <button class="ai-ghost" type="button" :disabled="disabled" @click="pickDir">选择…</button>
    </div>
  </div>

  <p v-if="privacy !== false" class="ai-privacy">
    本功能会把所选照片发送到你在「模型设置」中配置的第三方 API 服务处理，照片不会上传到本项目服务器；产物写入你选择的本地目录。
  </p>
</template>
