<script setup lang="ts">
/**
 * 本地模型状态 / 下载（FEAT-163 / FEAT-164 / FEAT-165）
 *
 * 列出工具所需的大模型是否就绪；缺失时**在应用内直接下载**（复用扫描那套
 * `ModelDownloadRows`：多镜像、断点续传、进度、取消、一键补全），
 * 不再「打开浏览器 + 手动放进模型目录」。
 *
 * 两边 id 不是一套命名：Python 侧（`/local/models`）用 `real_esrgan_x4` / `gfpgan_1.4`，
 * Rust 下载表（`model_dl`）用 `local-esrgan` / `local-gfpgan` —— 映射集中在本文件，
 * 工具组件继续按 Python id 传参，互不感知对方命名。
 *
 * FEAT-165：列表固定按 `area="workshop"` 取，扫描侧的识别模型不会漏进来。
 */
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAiStudioStore } from "../../../stores/aiStudio";
import ModelDownloadRows from "../../settings/ModelDownloadRows.vue";

const props = defineProps<{ ids?: string[] }>();
const store = useAiStudioStore();

/** Python 资源 id → Rust 下载注册名 */
const DL_NAMES: Record<string, string> = {
  real_esrgan_x4: "local-esrgan",
  "gfpgan_1.4": "local-gfpgan",
  face_parsing_r34: "local-faceparse",
  face_landmark_68: "local-landmark",
  paddleocr_rec: "local-ocr-rec",
};

const dlNames = computed(() => {
  const ids = props.ids ?? Object.keys(DL_NAMES);
  return ids.map((id) => DL_NAMES[id]).filter((n): n is string => !!n);
});

const modelsDir = ref("");

async function openDir() {
  try {
    if (!modelsDir.value) modelsDir.value = (await store.localModels()).modelsDir;
    if (!modelsDir.value) return;
    await invoke("open_folder", { path: modelsDir.value });
  } catch {
    /* 静默：目录未知时不打断用户（下载入口本身就是最常用的路径） */
  }
}
</script>

<template>
  <div class="ai-panel">
    <div class="ai-row" style="justify-content: space-between">
      <label class="ai-label">本地模型</label>
      <button class="ai-ghost" type="button" @click="openDir">打开模型目录</button>
    </div>

    <ModelDownloadRows area="workshop" :only-names="dlNames" />

    <p class="ai-note" style="margin-top: 6px">
      下载完成后立即可用；断点续传与多镜像择优由应用内下载器负责，不需要手动解压或改名。
    </p>
  </div>
</template>
