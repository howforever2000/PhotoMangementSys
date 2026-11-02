<script setup lang="ts">
/**
 * 轨迹：每步的缩略图 / 耗时 / 成本 / 说明 + 冲突与假设 + 产物。
 *
 * 为什么这么"啰嗦"（docs/v5.md §17 第 5、6 条）：Agent 与用户的信任建立在
 * **可审查**上。用户要能回答三个问题：
 *   1. 这次走的是哪条路（有没有花钱让模型理解）？
 *   2. 我写的那句话被理解成了什么（P1a 映射 / 冲突裁决）？
 *   3. 产物在哪、体检过没有？
 */
import { computed } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { AgentSnapshot } from "../../../types/agent";

const props = defineProps<{ job: AgentSnapshot | null }>();

const TOOL_LABEL: Record<string, string> = {
  plan: "规划",
  select: "选图",
  compress: "压缩",
  style_transfer: "本地调色",
  beauty_render: "人像润色",
  subject_extract: "抠主体",
  card_render: "卡片排版",
  probe: "产物体检",
  done: "完成",
};

const P1_LABEL: Record<string, string> = {
  P0: "P0 纯预设（未调用模型）",
  P1a: "P1a 规则映射（零成本，未出网）",
  P1b: "P1b 模型填参",
  P1c: "P1c 预设不读描述",
  P2: "P2 完全自定义",
  P3: "P3 你手改的提示词",
};

const pathLabel = computed(() => (props.job?.p1Path ? P1_LABEL[props.job.p1Path] ?? props.job.p1Path : ""));

const statusLabel = computed(() => {
  const s = props.job?.status;
  return (
    {
      starting: "启动中",
      running: "执行中",
      done: "已完成",
      error: "失败",
      cancelled: "已取消",
      budget: "已超预算中止",
      handoff: "已交接",
    } as Record<string, string>
  )[s ?? ""] ?? "";
});

const stepCount = computed(() => props.job?.steps.length ?? 0);

function fileName(p: string): string {
  const parts = p.split(/[\\/]/);
  return parts[parts.length - 1] || p;
}

/** 本地图必须经 convertFileSrc（WebView 不能直接读文件系统路径） */
function thumbSrc(p: string): string {
  return p ? convertFileSrc(p) : "";
}

function dirName(p: string): string {
  const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
  return i > 0 ? p.slice(0, i) : p;
}

async function openOutputs() {
  const first = props.job?.outputs?.[0];
  if (!first) return;
  try {
    await invoke("open_folder", { path: dirName(first) });
  } catch {
    // 目录被删/权限不足：不是致命问题，用户能从路径看出来
  }
}
</script>

<template>
  <section class="trail">
    <p v-if="!job" class="trail-empty">还没有任务。选好预设、把照片放进托盘，点「开始」。</p>

    <template v-else>
      <header class="trail-head">
        <span class="trail-status" :class="job.status">{{ statusLabel }}</span>
        <span v-if="pathLabel" class="trail-path">{{ pathLabel }}</span>
        <span class="trail-meta">{{ stepCount }} 步 · ¥{{ job.costTotal.toFixed(2) }}</span>
      </header>

      <!-- 冲突：用户明确说了相反的要求，这里必须显眼 -->
      <p v-for="(c, i) in job.conflicts" :key="`c${i}`" class="trail-conflict">{{ c }}</p>

      <!-- P1a 未命中：告诉用户"你这句话没生效"，并给出下一步 -->
      <p v-if="job.hint" class="trail-hint">{{ job.hint }}</p>

      <p v-if="job.error" class="trail-error">{{ job.error }}</p>

      <ol class="steps">
        <li v-for="s in job.steps" :key="s.idx" class="step">
          <span class="step-idx">{{ s.idx }}</span>
          <img v-if="s.thumb" class="step-thumb" :src="thumbSrc(s.thumb)" alt="" />
          <span v-else class="step-thumb step-thumb-empty" aria-hidden="true" />
          <span class="step-body">
            <span class="step-tool">{{ TOOL_LABEL[s.tool] ?? s.tool }}</span>
            <span v-if="s.latencyMs" class="step-ms">{{ s.latencyMs }}ms</span>
            <span v-if="s.cost" class="step-cost">¥{{ s.cost.toFixed(3) }}</span>
            <span v-if="s.note" class="step-note">{{ s.note }}</span>
          </span>
        </li>
      </ol>

      <!-- 假设/降级：默认收起，出问题时才展开看（不制造噪声） -->
      <details v-if="job.assumptions.length" class="trail-notes">
        <summary>执行假设与降级（{{ job.assumptions.length }}）</summary>
        <ul>
          <li v-for="(a, i) in job.assumptions" :key="`a${i}`">{{ a }}</li>
        </ul>
      </details>

      <div v-if="job.outputs.length" class="outs">
        <div class="outs-head">
          <span>产物 {{ job.outputs.length }} 个</span>
          <button class="outs-open" type="button" @click="openOutputs">打开所在文件夹</button>
        </div>
        <ul class="outs-list">
          <li v-for="o in job.outputs.slice(0, 6)" :key="o" :title="o">{{ fileName(o) }}</li>
        </ul>
        <p v-if="job.outputs.length > 6" class="outs-more">…另有 {{ job.outputs.length - 6 }} 个</p>
      </div>
    </template>
  </section>
</template>

<style scoped>
.trail {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.trail-empty {
  margin: 0;
  font-size: 12px;
  color: var(--fg-2, #a8a29a);
}

.trail-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  flex-wrap: wrap;
  padding-bottom: 6px;
  border-bottom: 1px solid var(--line, rgba(255, 255, 255, 0.1));
}

.trail-status {
  font-size: 12.5px;
  font-weight: 600;
  color: var(--fg, #ece7df);
}

.trail-status.done {
  color: #4caf7d;
}

.trail-status.error,
.trail-status.budget {
  color: #d9534f;
}

.trail-status.cancelled {
  color: var(--fg-2, #a8a29a);
}

.trail-path {
  font-size: 11px;
  color: var(--gold, #c8a45c);
}

.trail-meta {
  margin-left: auto;
  font-size: 11px;
  color: var(--fg-2, #a8a29a);
}

.trail-conflict {
  margin: 0;
  padding: 6px 9px;
  border-left: 3px solid var(--gold, #c8a45c);
  background: rgba(200, 164, 92, 0.09);
  border-radius: 0 5px 5px 0;
  font-size: 11.5px;
  line-height: 1.5;
  color: var(--fg, #ece7df);
}

.trail-hint {
  margin: 0;
  font-size: 11.5px;
  line-height: 1.55;
  color: var(--fg-2, #a8a29a);
}

.trail-error {
  margin: 0;
  font-size: 11.5px;
  line-height: 1.5;
  color: #d9534f;
}

.steps {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.step {
  display: flex;
  align-items: flex-start;
  gap: 7px;
  font-size: 11.5px;
}

.step-idx {
  flex: none;
  width: 14px;
  text-align: right;
  color: var(--fg-3, #6f6a63);
  font-variant-numeric: tabular-nums;
}

.step-thumb {
  flex: none;
  width: 30px;
  height: 30px;
  border-radius: 4px;
  object-fit: cover;
  background: var(--hover, rgba(255, 255, 255, 0.06));
}

.step-thumb-empty {
  display: block;
}

.step-body {
  display: flex;
  align-items: baseline;
  gap: 6px;
  flex-wrap: wrap;
  min-width: 0;
}

.step-tool {
  color: var(--fg, #ece7df);
  font-weight: 600;
}

.step-ms,
.step-cost {
  color: var(--fg-3, #6f6a63);
  font-variant-numeric: tabular-nums;
}

.step-note {
  flex-basis: 100%;
  color: var(--fg-2, #a8a29a);
  line-height: 1.5;
  word-break: break-all;
}

.trail-notes {
  font-size: 11px;
  color: var(--fg-2, #a8a29a);
}

.trail-notes summary {
  cursor: pointer;
}

.trail-notes ul {
  margin: 5px 0 0;
  padding-left: 16px;
  line-height: 1.6;
}

.outs {
  padding-top: 6px;
  border-top: 1px solid var(--line, rgba(255, 255, 255, 0.1));
}

.outs-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11.5px;
  color: var(--fg-2, #a8a29a);
}

.outs-open {
  padding: 2px 8px;
  border: 1px solid var(--line, rgba(255, 255, 255, 0.16));
  border-radius: 5px;
  background: none;
  color: var(--fg-2, #a8a29a);
  font-size: 11px;
  cursor: pointer;
}

.outs-open:hover {
  color: var(--fg, #ece7df);
}

.outs-list {
  margin: 5px 0 0;
  padding-left: 16px;
  font-size: 11px;
  color: var(--fg-2, #a8a29a);
  line-height: 1.7;
  word-break: break-all;
}

.outs-more {
  margin: 2px 0 0;
  font-size: 11px;
  color: var(--fg-3, #6f6a63);
}
</style>
