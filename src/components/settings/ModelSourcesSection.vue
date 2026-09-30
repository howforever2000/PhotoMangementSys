<script setup lang="ts">
/**
 * 📦 模型管理 · 下载源治理（FEAT-061）
 *
 * URL 不可达时不再静默挂起：一键自检各源连通性 + 自定义源模板（自定义优先于内置）。
 * 独立成组件是为了与「下载进度」各管一摊 —— 源治理是网络配置，下载是任务执行。
 */
import { computed, ref } from "vue";
import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import type { ModelSourceProbe } from "../../types/content";

const store = useContentStore();
const notify = useNotify();

const probes = ref<ModelSourceProbe[]>([]);
const probeBusy = ref(false);
const probeErr = ref("");
const srcOpen = ref(false);
const customSrc = ref("");
const srcSaving = ref(false);
const probeTarget = ref("");

/** 探测对象：优先拿第一个还没下的（它最需要换源），没有就用清单第一个 */
const targetName = computed(() => {
  const list = store.modelDownloads;
  return (list.find((d) => !d.done) ?? list[0])?.name ?? "";
});

async function runProbe() {
  if (!targetName.value) return;
  probeTarget.value = targetName.value;
  probeBusy.value = true;
  probeErr.value = "";
  try {
    probes.value = await store.probeModelSources(targetName.value);
  } catch (e) {
    probes.value = [];
    probeErr.value = String(e);
  } finally {
    probeBusy.value = false;
  }
}

async function toggleSources() {
  srcOpen.value = !srcOpen.value;
  if (!srcOpen.value) return;
  try {
    const info = await store.getModelSources();
    customSrc.value = info.custom.join("\n");
  } catch (e) {
    notify.error("读取下载源配置失败", String(e));
  }
}

async function saveSources() {
  srcSaving.value = true;
  try {
    const list = customSrc.value
      .split(/\r?\n/)
      .map((s) => s.trim())
      .filter(Boolean);
    await store.setModelSources(list);
    notify.success(
      "下载源已保存",
      list.length ? `自定义 ${list.length} 个源（下载时优先尝试）` : "已恢复为内置镜像",
    );
    await runProbe();
  } catch (e) {
    notify.error("保存下载源失败", String(e));
  } finally {
    srcSaving.value = false;
  }
}
</script>

<template>
  <div class="src-bar">
    <button class="btn btn-sm" :disabled="probeBusy || !targetName" @click="runProbe">
      {{ probeBusy ? "检测中…" : "🔍 镜像源自检" }}
    </button>
    <button class="btn btn-sm" @click="toggleSources">
      {{ srcOpen ? "收起自定义源" : "自定义源" }}
    </button>
    <span v-if="probes.length" class="stat dim">
      {{ probeTarget }} · 可用 {{ probes.filter((p) => p.ok).length }} / {{ probes.length }}
    </span>
    <span class="hint">下载卡住/超时先点这里，能直接看出是哪个源不通</span>
  </div>

  <p v-if="probeErr" class="stat err">{{ probeErr }}</p>

  <div v-if="probes.length" class="probe-list">
    <div v-for="p in probes" :key="p.url" class="probe-row">
      <span class="probe-host" :title="p.url">{{ p.host }}</span>
      <span class="probe-tag" :class="p.ok ? 'ok' : 'bad'">
        {{ p.ok ? `可用 ${p.status}` : p.status ? `HTTP ${p.status}` : "不可用" }}
      </span>
      <span class="probe-ms">{{ p.ms }} ms</span>
      <span class="probe-note" :title="p.error ?? ''">
        {{ p.error ? p.error.slice(0, 70) : p.builtin ? "内置源" : "自定义源" }}
      </span>
    </div>
  </div>

  <div v-if="srcOpen" class="src-edit">
    <p class="hint">
      每行一个模板，必须含 <code>{repo}</code> 与 <code>{path}</code>；自定义源优先于内置源，下载按顺序尝试。
    </p>
    <textarea
      v-model="customSrc"
      class="src-text"
      spellcheck="false"
      placeholder="https://your-mirror.example.com/{repo}/resolve/main/{path}"
    ></textarea>
    <div class="src-actions">
      <button class="btn btn-sm" :disabled="srcSaving" @click="saveSources">
        {{ srcSaving ? "保存中…" : "保存并检测" }}
      </button>
      <button class="btn btn-sm" :disabled="srcSaving" @click="customSrc = ''">清空自定义</button>
    </div>
  </div>
</template>

<style scoped>
.src-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.probe-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.probe-row {
  display: grid;
  grid-template-columns: minmax(90px, auto) 86px 70px 1fr;
  gap: 8px;
  font-size: 12px;
  align-items: center;
}
.probe-host {
  color: var(--color-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.probe-tag.ok {
  color: var(--color-ok);
}
.probe-tag.bad {
  color: var(--color-danger);
}
.probe-ms {
  font-family: Consolas, monospace;
  color: var(--color-text-3);
}
.probe-note {
  color: var(--color-text-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.src-edit {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.src-text {
  min-height: 74px;
  padding: 8px 10px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm, 8px);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-family: Consolas, monospace;
  font-size: 12px;
  resize: vertical;
}
.src-actions {
  display: flex;
  gap: 8px;
}
.hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--color-text-3);
}
.stat {
  font-size: 12.5px;
  color: var(--color-text-2);
}
.stat.dim {
  color: var(--color-text-3);
}
.stat.err {
  color: var(--color-danger);
}
.btn {
  min-height: 26px;
  padding: 3px 10px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm, 8px);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}
.btn:hover:not(:disabled) {
  border-color: var(--color-primary);
  color: var(--color-primary);
}
.btn:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--color-primary) 55%, transparent);
  outline-offset: 2px;
}
.btn:disabled {
  background: var(--color-neutral-soft);
  color: var(--color-text-3);
  cursor: not-allowed;
}
</style>
