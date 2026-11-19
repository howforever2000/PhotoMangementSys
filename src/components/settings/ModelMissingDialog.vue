<script setup lang="ts">
/**
 * 模型缺失弹窗（FEAT-114）
 *
 * 触发场景：语义搜索发现 CLIP 未下载、扫描前人脸通道预检报「缺少模型文件」。
 * 目标：把「后台反复重试 + 只给一句报错」变成「明确告诉你缺什么 + 当场可下载」。
 *
 * BUG-2026-1028-011：新增 `det`（人物检测）场景 —— 0.4.x 轻量安装包不带模型，
 * 而下载表曾漏登记 yolov8n-det.onnx，于是「人物通道不可用」根本没办法自救；
 * 现在缺它时直接弹到这里，列表里第一行就是「人物检测 · YOLOv8n（必需）」。
 *
 * 列表与设置页共用 ModelDownloadRows；这里只负责场景化文案与兜底通道。
 *
 * FEAT-165：本弹窗是**识别链路**的缺模型自救入口，因此列表按 `area="scan"` 只列
 * 识别模型 —— 创意工坊的本地增强模型不在这里出现（它们去工坊里下），
 * 下面的「已全部就绪」也只按识别链路判定，否则本地模型没下就永远等不到这句。
 */
import { computed } from "vue";
import { useThemeStore } from "../../stores/theme";
import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import { modelsInArea } from "../../utils/modelCatalog";
import {
  OFFLINE_PACKAGE_ARCHIVE_ROOT,
  offlinePackageSizeText,
  openOfflinePackagePage,
} from "../../utils/modelOfflinePackage";
import ModelDownloadRows from "./ModelDownloadRows.vue";

const props = defineProps<{
  visible: boolean;
  /** 触发来源：语义搜索 / 人物检测 / 人脸扫描 / OCR / 通用 */
  reason?: "semantic" | "det" | "face" | "ocr" | "general";
  /** 缺哪些文件（后端报错原文里的文件名，展示用） */
  missingFiles?: string[];
}>();

const emit = defineEmits<{ close: [] }>();

const theme = useThemeStore();
const store = useContentStore();
const notify = useNotify();

/**
 * 兜底通道的 URL 与体积**不再在本文件定义**（BUG-2026-1119-002）：
 * 这里曾与 ModelDownloadsSection 各写一份 `v0.4.5/model.zip` + 「529MB」，
 * 而实测识别链路已是 979MB。文案与地址统一从 utils/modelOfflinePackage.ts 取。
 */

const title = computed(() => {
  if (props.reason === "det") return "人物检测模型缺失";
  if (props.reason === "face") return "人脸模型缺失";
  if (props.reason === "ocr") return "OCR 模型缺失";
  if (props.reason === "semantic") return "语义模型缺失";
  return "模型缺失";
});

const reasonText = computed(() => {
  if (props.reason === "semantic") {
    return "CLIP 语义模型没有下载，智能搜索只能做关键词匹配。下载后无需重启，搜索会自动恢复语义检索。";
  }
  if (props.reason === "det") {
    return "人物检测模型（YOLOv8n）没有下载。它是识别主链路，缺它时整条人物 / 扫描链路都不就绪（表现为扫描没反应、人物出不来）。下载 12.8MB 后重试扫描即可。";
  }
  if (props.reason === "face") {
    return "人脸检测 / 识别模型没有下载，扫描会跳过人脸登记（表现为「扫完 0 位人物」）。请补齐当前档位的两个模型。";
  }
  if (props.reason === "ocr") {
    return "PaddleOCR 检测模型没有下载，文档 / 截图里的文字无法识别。下载后重新扫描即可。";
  }
  return "识别通道所需的模型文件不完整，对应功能会静默降级。";
});

/** 只按识别链路模型判断就绪：工坊的本地增强模型与本弹窗的场景无关（FEAT-165） */
const scanModels = computed(() => modelsInArea(store.modelDownloads, "scan"));
const allReady = computed(
  () => scanModels.value.length > 0 && scanModels.value.every((d) => d.done),
);

async function refresh() {
  try {
    await store.listModelDownloads();
  } catch (e) {
    notify.error("刷新模型状态失败", String(e));
  }
}

async function openModelZip() {
  try {
    await openOfflinePackagePage();
  } catch (e) {
    notify.error("打开下载页失败", String(e));
  }
}

function open() {
  void refresh();
}
defineExpose({ open });
</script>

<template>
  <Teleport to="body">
    <div v-if="visible" class="pm-modal" @click.self="emit('close')">
      <div
        class="pm-dialog pm-dialog-wide mmd"
        role="dialog"
        aria-modal="true"
        aria-labelledby="mmd-title"
        :style="theme.dialogVarStyle"
      >
        <div class="pm-dialog-head">
          <h3 id="mmd-title">📦 {{ title }}</h3>
          <span class="pm-hint">下载在后台进行，可随时关闭此窗口</span>
        </div>

        <p class="mmd-reason">{{ reasonText }}</p>
        <p v-if="missingFiles?.length" class="mmd-files pm-mono">
          缺少文件：{{ missingFiles.join("、") }}
        </p>

        <p v-if="allReady" class="mmd-ok">✓ 模型已全部就绪，可以关闭窗口继续使用。</p>

        <ModelDownloadRows area="scan" />

        <p class="mmd-fallback">
          网络受限时，也可下载离线包
          <button class="mmd-link" type="button" @click="openModelZip">
            model.zip（{{ offlinePackageSizeText() }}）
          </button>
          ，解压到模型目录（包内已是 <code>{{ OFFLINE_PACKAGE_ARCHIVE_ROOT }}</code> 结构，
          不要再套一层）后重启应用。具体落点见「性能设置 → 模型管理」里的离线包说明。
        </p>

        <div class="pm-actions">
          <button class="pm-btn" type="button" @click="refresh">重新检测</button>
          <button class="pm-btn pm-btn-primary" type="button" @click="emit('close')">关闭</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.mmd {
  width: min(680px, 100%);
}
.mmd-reason {
  margin: 0 0 8px;
  font-size: 12.5px;
  line-height: 1.75;
  color: var(--pm-label);
}
.mmd-files {
  margin: 0 0 10px;
  padding: 6px 8px;
  border-radius: 8px;
  background: var(--pm-input-bg);
  font-size: 11.5px;
  word-break: break-all;
  color: var(--pm-hint);
}
.mmd-ok {
  margin: 0 0 8px;
  font-size: 12.5px;
  color: var(--color-ok);
}
.mmd-fallback {
  margin: 12px 0 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--pm-hint);
}
.mmd-link {
  padding: 0;
  border: 0;
  background: none;
  color: var(--color-primary-text);
  font-size: 12px;
  text-decoration: underline;
  cursor: pointer;
}
.mmd-fallback code {
  padding: 1px 4px;
  border-radius: 4px;
  background: var(--pm-input-bg);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 11px;
}
</style>
