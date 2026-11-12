<script setup lang="ts">
/**
 * 本地模型状态 / 下载（FEAT-163）
 * 列出某工具所需的大模型是否就绪，缺失时给下载链接（GitHub release）+ 打开模型目录。
 */
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useAiStudioStore } from "../../../stores/aiStudio";
import type { LocalModelEntry } from "../../../types/ai";

const props = defineProps<{ ids?: string[] }>();
const store = useAiStudioStore();

const models = ref<LocalModelEntry[]>([]);
const modelsDir = ref("");
const loading = ref(true);
const err = ref("");

async function load() {
  loading.value = true;
  err.value = "";
  try {
    const st = await store.localModels();
    models.value = props.ids ? st.models.filter((m) => props.ids!.includes(m.id)) : st.models;
    modelsDir.value = st.modelsDir;
  } catch (e) {
    err.value = String(e);
  } finally {
    loading.value = false;
  }
}
onMounted(load);

async function download(m: LocalModelEntry) {
  try {
    await openUrl(m.url);
  } catch {
    /* 打不开就静默，用户可手动复制地址 */
  }
}

async function openDir() {
  try {
    await invoke("open_folder", { path: modelsDir.value });
  } catch {
    /* 静默 */
  }
}
</script>

<template>
  <div class="ai-panel">
    <div class="ai-row" style="justify-content: space-between">
      <label class="ai-label">本地模型</label>
      <button class="ai-ghost" type="button" @click="openDir">打开模型目录</button>
    </div>

    <p v-if="loading" class="ai-note">正在检查模型…</p>
    <p v-else-if="err" class="ai-note ai-warn">{{ err }}</p>
    <template v-else>
      <div v-for="m in models" :key="m.id" class="ai-out-row" style="border-top: none; padding: 2px 0">
        <span class="ai-out-name" :title="m.name">{{ m.name }}</span>
        <span class="ai-dim" style="font-size: 12px">约 {{ m.sizeMb }}MB</span>
        <span v-if="m.exists" class="ai-ok">已就绪</span>
        <template v-else>
          <span class="ai-warn">未下载</span>
          <button class="ai-ghost" type="button" @click="download(m)">下载 ↗</button>
        </template>
      </div>
      <p class="ai-note">用浏览器下载后，把文件放到「模型目录」，回到本页重新进入即可检测到。</p>
    </template>
  </div>
</template>

<style scoped>
.ai-ok {
  color: var(--c-success, #3f9d52);
  font-size: 12px;
}
</style>
