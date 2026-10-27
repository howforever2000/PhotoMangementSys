<script setup lang="ts">
/**
 * 📦 模型管理 · 下载（FEAT-052 / FEAT-114）
 *
 * 后台下载 + 进度 + 官方/镜像择优；列表本身抽到 ModelDownloadRows，
 * 与「模型缺失」弹窗共用（两处不各写一套进度/重试逻辑）。
 * **不依赖识别服务** —— 服务没起来也能下。
 */
import ModelDownloadRows from "./ModelDownloadRows.vue";
import { useNotify } from "../../composables/useNotify";

const notify = useNotify();

/** 兜底通道：网络受限时的离线包（README 里同一份地址） */
const MODEL_ZIP =
  "https://github.com/howforever2000/PhotoMangementSys/releases/download/v0.4.2/model.zip";

async function openModelZip() {
  try {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(MODEL_ZIP);
  } catch (e) {
    notify.error("打开下载页失败", String(e));
  }
}
</script>

<template>
  <div class="dl-head">
    <h4 class="sub-title">📥 模型下载</h4>
    <span class="hint">
      语义 / 人脸 / OCR 均可在此直接下载 · 多镜像候选逐个尝试、失败自动换源 · 支持断点续传
    </span>
  </div>

  <ModelDownloadRows />

  <p class="hint fallback">
    网络下载总是失败？可下载离线包
    <button class="link" type="button" @click="openModelZip">model.zip（约 529MB）</button>
    解压到安装根目录（MSI 只读安装为
    <code>%APPDATA%\com.photomanagementsys.app\vcr-models</code>）后重启应用。
  </p>
</template>

<style scoped>
.dl-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
}
.sub-title {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  color: var(--color-text);
}
.hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--color-text-3);
}
.fallback {
  margin-top: 6px;
}
.link {
  padding: 0;
  border: 0;
  background: none;
  color: var(--color-primary-text);
  font-size: 12px;
  text-decoration: underline;
  cursor: pointer;
}
code {
  padding: 1px 4px;
  border-radius: 4px;
  background: var(--color-neutral-soft);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
</style>
