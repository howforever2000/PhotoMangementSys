<script setup lang="ts">
/**
 * 📦 模型管理 · 下载（FEAT-052 / FEAT-114 / FEAT-165 / BUG-2026-1119-002）
 *
 * 后台下载 + 进度 + 官方/镜像择优；列表本身抽到 ModelDownloadRows，
 * 与「模型缺失」弹窗共用（两处不各写一套进度/重试逻辑）。
 * **不依赖识别服务** —— 服务没起来也能下。
 *
 * FEAT-165：这里只列**识别链路**模型（area="scan"）。创意工坊的本地增强模型
 * 走同一个下载器，但入口在创意工坊里（模型设置 › 本地增强模型 / 画质修复面板），
 * 免得扫描侧列表里混进与扫描无关的超分 / 修脸模型。
 *
 * BUG-2026-1119-002：离线包这段文案与链接原先在本文件与 ModelMissingDialog 各写
 * 一份，且 URL 钉死 `v0.4.5`、体积停在 v0.4.2 的「529MB」（实测识别链路 979MB）。
 * 现在全部从 `utils/modelOfflinePackage.ts` 读 —— 那个文件是唯一事实源，
 * 本组件只渲染，不持有任何数字或地址。
 */
import { ref } from "vue";
import ModelDownloadRows from "./ModelDownloadRows.vue";
import { useNotify } from "../../composables/useNotify";
import {
  OFFLINE_PACKAGE_ARCHIVE_ROOT,
  OFFLINE_PACKAGE_CONTENTS,
  OFFLINE_PACKAGE_TARGETS,
  offlinePackageSizeText,
  openOfflinePackagePage,
  openModelsDir,
} from "../../utils/modelOfflinePackage";

const notify = useNotify();

/** 解压说明默认收起：它是兜底路径，不该和「一键下载」抢注意力 */
const showFallback = ref(false);

async function downloadOffline() {
  try {
    await openOfflinePackagePage();
  } catch (e) {
    notify.error("打开下载页失败", String(e));
  }
}

/**
 * 直接打开**当前生效**的模型目录（后端 `model_dir` 命令，与扫描/Python 同源）。
 *
 * 离线包的正确落点在三种安装形态下不同，与其让用户照着文字找路径，不如把
 * 资源管理器开在他该解压进去的位置。目录拿不到才退回文字说明。
 */
async function openDir() {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const dir = await invoke<string>("model_dir");
    await openModelsDir(dir);
  } catch (e) {
    notify.error("打开模型目录失败", String(e));
  }
}
</script>

<template>
  <div class="dl-head">
    <h4 class="sub-title">📥 模型下载</h4>
    <span class="hint">
      语义 / 人脸 / OCR / 人物检测均可在此直接下载 · 多镜像候选逐个尝试、失败自动换源 · 支持断点续传
    </span>
  </div>

  <ModelDownloadRows area="scan" />

  <!-- 兜底通道（BUG-2026-1119-002）：网络受限时的离线包。
       体积与 URL 都来自 utils/modelOfflinePackage.ts，不在这里写第二份。 -->
  <div class="fallback-box">
    <button class="fallback-head" type="button" @click="showFallback = !showFallback">
      <span class="caret">{{ showFallback ? "▾" : "▸" }}</span>
      <span>网络下载总是失败？用离线包（{{ offlinePackageSizeText() }}）</span>
    </button>

    <div v-if="showFallback" class="fallback-body">
      <p class="hint">
        下载
        <button class="link" type="button" @click="downloadOffline">model.zip</button>
        后解压。包内已是
        <code>{{ OFFLINE_PACKAGE_ARCHIVE_ROOT }}</code> 结构：把其中<strong>之下的文件</strong>
        解压到下面这个目录（不要再套一层 <code>vcr\models</code>）。解压完重启应用即可（程序只在启动时扫描模型目录）。
      </p>

      <button class="btn-dir" type="button" @click="openDir">📂 打开我的模型目录</button>

      <dl class="targets">
        <template v-for="t in OFFLINE_PACKAGE_TARGETS" :key="t.key">
          <dt>{{ t.title }}</dt>
          <dd>
            <code>{{ t.path }}</code>
            <span class="why">{{ t.why }}</span>
          </dd>
        </template>
      </dl>

      <p class="hint">
        <strong>不确定哪种？</strong>直接点上面的「打开我的模型目录」—— 程序会打开它
        正在读取的那一个，解压进去就对了。
      </p>

      <p class="hint">离线包覆盖：
        <span v-for="(c, i) in OFFLINE_PACKAGE_CONTENTS" :key="c">
          {{ c }}<span v-if="i < OFFLINE_PACKAGE_CONTENTS.length - 1"> · </span>
        </span>
      </p>
      <p class="hint dim">
        创意工坊的本地增强模型（超分 / 人脸修复 / 人脸解析 / 关键点 / OCR 识别）
        不在这个包里，它们走应用内下载：创意工坊 → 模型设置 → 本地增强模型。
      </p>
    </div>
  </div>
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
.hint.dim {
  opacity: 0.8;
}

/* 兜底通道：默认收起的一行，展开后才占版面 */
.fallback-box {
  margin-top: 8px;
  border-top: 1px dashed var(--color-border);
  padding-top: 6px;
}
.fallback-head {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 4px 0;
  border: none;
  background: none;
  color: var(--color-text-2);
  font-size: 12px;
  text-align: left;
  cursor: pointer;
}
.fallback-head:hover {
  color: var(--color-text);
}
.caret {
  width: 1em;
  color: var(--color-primary-text);
}
.fallback-body {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 4px 0 2px;
}

/* 落点清单：三种安装形态并排，用户按自己的安装方式对号入座 */
.targets {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  gap: 4px 10px;
  margin: 2px 0;
  font-size: 12px;
}
.targets dt {
  color: var(--color-text-2);
  white-space: nowrap;
}
.targets dd {
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}
.why {
  font-size: 11px;
  color: var(--color-text-3);
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
.btn-dir {
  align-self: flex-start;
  min-height: 28px;
  padding: 4px 12px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm, 8px);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}
.btn-dir:hover {
  border-color: var(--color-primary);
  color: var(--color-primary-text);
}
code {
  padding: 1px 4px;
  border-radius: 4px;
  /* 不透明底：--color-neutral-soft 在深色档也够深，不会把 code 变成白块 */
  background: var(--popup-bg, #ffffff);
  color: var(--popup-fg, #1f2733);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 11px;
}
</style>
