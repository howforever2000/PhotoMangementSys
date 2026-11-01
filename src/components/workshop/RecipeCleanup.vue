<script setup lang="ts">
/**
 * 旧调色配方清理（FEAT-150）
 * ===================================================================
 * 背景：v0.4.4 起创意工坊不再提供调色编辑，旧引擎写在照片同目录的
 * `*.pmrecipe.json` / `*.pmmask.png` 从此不会被读取 —— 但文件还在用户磁盘上。
 *
 * 产品判断：**不自动删**。这些是用户目录里的文件，我们无权在升级时悄悄动手；
 * 但也不能装作没有，于是给一个显式的、可预览数量的入口。
 *
 * 删除走系统回收站，误删可找回（后端 `tools::purge_sidecars`）。
 */
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useNotify } from "../../composables/useNotify";
import type { SidecarPurgeOutcome, SidecarScan } from "../../types/tools";
import { formatBytes } from "../../utils/bytes";

const notify = useNotify();

const dir = ref("");
const scanning = ref(false);
const purging = ref(false);
const scan = ref<SidecarScan | null>(null);

async function pickDir() {
  const picked = await openDialog({
    directory: true,
    title: "选择照片所在的文件夹（会递归扫描子目录）",
  });
  if (typeof picked !== "string") return;
  dir.value = picked;
  scan.value = null;
  await doScan();
}

async function doScan() {
  if (!dir.value || scanning.value) return;
  scanning.value = true;
  try {
    scan.value = await invoke<SidecarScan>("recipe_sidecar_scan", { dir: dir.value });
  } catch (e) {
    notify.error("扫描失败", String(e));
  } finally {
    scanning.value = false;
  }
}

async function doPurge() {
  const s = scan.value;
  if (!s || !s.files.length || purging.value) return;
  const ok = await notify.confirm(
    "删除这些残留文件？",
    `将删除 ${s.recipeCount} 个配方文件与 ${s.maskCount} 个蒙版文件（共 ${formatBytes(
      s.totalBytes,
    )}）。它们已经不会被任何版本读取；文件会进系统回收站，可从回收站找回。`,
    { confirmText: "删除", cancelText: "取消", type: "danger" },
  );
  if (!ok) return;

  purging.value = true;
  try {
    const out = await invoke<SidecarPurgeOutcome>("recipe_sidecar_purge", { dir: dir.value });
    if (out.deleted) {
      notify.success(
        "清理完成",
        `已删除 ${out.deleted} 个文件，回收 ${formatBytes(out.freedBytes)}` +
          (out.failed ? `（${out.failed} 个失败）` : ""),
      );
    } else {
      notify.warning("没有删除任何文件", out.failedPaths[0] ?? "文件可能已被移走");
    }
    scan.value = null;
    await doScan();
  } catch (e) {
    notify.error("清理失败", String(e));
  } finally {
    purging.value = false;
  }
}
</script>

<template>
  <section class="rc">
    <div class="rc-head">
      <h3 class="rc-title">旧调色配方</h3>
      <span class="rc-sub">
        照片目录里残留的 <code>*.pmrecipe.json</code> / <code>*.pmmask.png</code> 已不再被读取，可一次性清掉
      </span>
    </div>

    <div class="rc-row">
      <span class="rc-path" :title="dir || '尚未选择'">{{ dir || "尚未选择文件夹" }}</span>
      <button class="btn btn-sm" type="button" :disabled="scanning || purging" @click="pickDir">
        {{ dir ? "换个文件夹" : "选择文件夹" }}
      </button>
      <button
        v-if="dir"
        class="btn btn-sm"
        type="button"
        :disabled="scanning || purging"
        @click="doScan"
      >
        {{ scanning ? "扫描中…" : "重新扫描" }}
      </button>
    </div>

    <div v-if="scan" class="rc-result">
      <template v-if="scan.files.length">
        <span class="rc-count">
          配方 {{ scan.recipeCount }} 个 · 蒙版 {{ scan.maskCount }} 个 ·
          <strong>{{ formatBytes(scan.totalBytes) }}</strong>
        </span>
        <span class="rc-scanned">扫了 {{ scan.scannedFiles }} 个文件</span>
        <button class="rc-purge" type="button" :disabled="purging" @click="doPurge">
          {{ purging ? "清理中…" : "删除这些残留" }}
        </button>
      </template>
      <span v-else class="rc-clean">
        这个文件夹里没有残留（扫了 {{ scan.scannedFiles }} 个文件）
      </span>
    </div>
  </section>
</template>

<style scoped>
.rc {
  margin-top: 18px;
  padding: 14px 16px;
  border-radius: var(--r-ui);
  border: 1px solid var(--c-rule);
}

.rc-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 10px;
}

/* 维护区标题比工具名弱一级：它是收尾事项，不该和工具抢注意力 */
.rc-title {
  margin: 0;
  font-size: var(--fs-title);
  font-weight: var(--fw-em);
  color: var(--fg);
}

.rc-sub {
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.5;
}

.rc-sub code {
  font-size: 10.5px;
  padding: 1px 4px;
  border-radius: 3px;
  background: var(--hover, rgba(120, 120, 130, 0.14));
}

.rc-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

/* 次级按钮统一到 30px 行高，与左侧路径框齐平（全局 .btn 是 34px，混在一行会高低不齐） */
.btn-sm {
  min-height: 30px;
  padding: 0 12px;
  font-size: var(--fs-meta);
}

.rc-path {
  flex: 1;
  min-width: 0;
  height: 30px;
  display: flex;
  align-items: center;
  padding: 0 8px;
  border-radius: 7px;
  border: 1px dashed var(--c-rule);
  font-size: var(--fs-meta);
  color: var(--fg-2);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.rc-result {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  margin-top: 10px;
  font-size: var(--fs-meta);
  color: var(--fg);
  font-variant-numeric: tabular-nums;
}

.rc-count strong {
  font-weight: var(--fw-strong);
}

.rc-scanned,
.rc-clean {
  color: var(--fg-2);
}

.rc-purge {
  height: 26px;
  padding: 0 10px;
  border: none;
  border-radius: var(--r-pill);
  background: none;
  color: var(--c-danger);
  font-size: var(--fs-meta);
  cursor: pointer;
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-danger) 45%, transparent);
  transition: background var(--dur-2) var(--ease-out);
}
.rc-purge:hover:not(:disabled) {
  background: color-mix(in srgb, var(--c-danger) 14%, transparent);
}
.rc-purge:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
