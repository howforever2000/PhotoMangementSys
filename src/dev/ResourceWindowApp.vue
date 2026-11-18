<script setup lang="ts">
/**
 * 开发者视角 · 资源使用情况（副窗口，label=dev-resources，FEAT-179 迭代）
 *
 * 只展示「与相册管理相关的程序」——主程序 / 视觉识别 VCR / AI 创作 /
 * 本地 AI 修图网关(8000) / 图像生成 sd-server(1234)，不再列全机进程。
 *
 * 内部启停（Q1-A/Q2-A/Q3-A）：
 *   - VCR / AI 创作：可启动、可停止，停止直接生效（有作业在跑时开发者自行承担）；
 *   - 网关 / sd-server：外部程序，App 不拥有，只显示状态与手动启动提示；
 *   - 主程序：只展示，不提供停止。
 * GPU 为识别服务配置态，网络占位，CPU/内存每 2s 轮询。
 */
import { computed, onMounted, onUnmounted } from "vue";
import { useResourceUsageStore } from "../stores/resourceUsage";
import type { ComponentKind } from "../types/resourceUsage";

const store = useResourceUsageStore();

const KIND_ICON: Record<ComponentKind, string> = {
  app: "🖼️",
  vcr: "🔍",
  ai_studio: "🎨",
  gateway: "🌐",
  sd_server: "🖥️",
};

const KIND_LABEL: Record<ComponentKind, string> = {
  app: "主程序",
  vcr: "视觉识别",
  ai_studio: "AI 创作",
  gateway: "外部网关",
  sd_server: "外部服务",
};

const components = computed(() => store.snapshot?.components ?? []);
const runningCount = computed(() => components.value.filter((c) => c.running).length);

function fmtBytes(n: number): string {
  if (!n) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

function fmtPct(n: number): string {
  if (!Number.isFinite(n)) return "—";
  return `${n.toFixed(1)}%`;
}

function fmtTime(ms: number): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleTimeString("zh-CN", { hour12: false });
}

function isBusy(key: string): boolean {
  return store.controlling === key;
}

onMounted(() => store.startPolling());
onUnmounted(() => store.stopPolling());
</script>

<template>
  <div class="ru">
    <header class="ru-head">
      <div class="ru-title">
        <span class="ru-dot" :class="{ live: store.polling }"></span>
        开发者视角 · 资源使用情况
        <span class="ru-tag">{{ store.polling ? "每 2s 自动刷新" : "已暂停" }}</span>
      </div>
      <div class="ru-actions">
        <span v-if="store.snapshot" class="ru-stamp">
          {{ fmtTime(store.snapshot.captured_at) }} · 间隔
          {{ store.snapshot.sample_interval_ms || "—" }}ms
        </span>
        <button class="ru-btn" type="button" :disabled="store.loading" @click="store.sample">
          {{ store.loading ? "采集中…" : "立即刷新" }}
        </button>
      </div>
    </header>

    <p v-if="store.error" class="ru-err">{{ store.error }}</p>
    <p v-else-if="store.notice" class="ru-notice">{{ store.notice }}</p>

    <!-- 摘要条 -->
    <div v-if="store.snapshot" class="ru-summary">
      <div class="ru-stat">
        <span class="ru-stat-label">系统 CPU</span>
        <b class="ru-stat-value">{{ fmtPct(store.snapshot.system_cpu_percent) }}</b>
      </div>
      <div class="ru-stat">
        <span class="ru-stat-label">系统内存</span>
        <b class="ru-stat-value">
          {{ fmtBytes(store.snapshot.system_memory_used) }}
          <small>/ {{ fmtBytes(store.snapshot.system_memory_total) }}</small>
        </b>
      </div>
      <div class="ru-stat">
        <span class="ru-stat-label">GPU（配置态）</span>
        <b class="ru-stat-value" :class="{ on: store.snapshot.gpu.enabled }">
          {{ store.snapshot.gpu.enabled ? "已启用" : "未启用 / CPU" }}
        </b>
        <span class="ru-stat-sub" :title="store.snapshot.gpu.note">
          {{ store.snapshot.gpu.provider || store.snapshot.gpu.note }}
        </span>
      </div>
      <div class="ru-stat">
        <span class="ru-stat-label">相关程序</span>
        <b class="ru-stat-value">{{ runningCount }} / {{ components.length }}</b>
        <span class="ru-stat-sub">运行中 / 总数</span>
      </div>
    </div>

    <!-- 组件卡片 -->
    <div class="ru-cards">
      <article
        v-for="c in components"
        :key="c.key"
        class="ru-card"
        :class="{ offline: !c.running, owned: c.can_stop }"
      >
        <div class="ru-card-head">
          <span class="ru-icon">{{ KIND_ICON[c.kind] }}</span>
          <b class="ru-card-name">{{ c.name }}</b>
          <span class="ru-badge" :class="c.running ? 'on' : 'off'">
            {{ c.running ? "运行中" : "未运行" }}
          </span>
          <span class="ru-kind">{{ KIND_LABEL[c.kind] }}</span>
        </div>

        <div class="ru-metrics">
          <div class="ru-metric">
            <span>PID</span>
            <b>{{ c.pid ?? "—" }}</b>
          </div>
          <div class="ru-metric">
            <span>CPU</span>
            <b>{{ c.running ? fmtPct(c.cpu_percent) : "—" }}</b>
          </div>
          <div class="ru-metric">
            <span>内存</span>
            <b>{{ c.running ? fmtBytes(c.memory_bytes) : "—" }}</b>
          </div>
          <div class="ru-metric">
            <span>内存%</span>
            <b>{{ c.running ? fmtPct(c.memory_percent) : "—" }}</b>
          </div>
          <div class="ru-metric">
            <span>网络</span>
            <b class="dim">—</b>
          </div>
        </div>

        <p class="ru-note">{{ c.note }}</p>

        <div class="ru-card-foot">
          <template v-if="c.can_stop || c.can_start">
            <button
              v-if="c.can_start"
              class="ru-btn small"
              type="button"
              :disabled="c.running || !!store.controlling"
              @click="store.control(c.key, 'start')"
            >
              {{ isBusy(c.key) ? "启动中…" : "启动" }}
            </button>
            <button
              v-if="c.can_stop"
              class="ru-btn small danger"
              type="button"
              :disabled="!c.running || !!store.controlling"
              @click="store.control(c.key, 'stop')"
            >
              {{ isBusy(c.key) ? "停止中…" : "停止" }}
            </button>
          </template>
          <span v-else class="ru-ext-hint">外部程序 · 需手动启动，App 不拥有启停权</span>
        </div>
      </article>
    </div>

    <p v-if="store.snapshot" class="ru-foot" :title="store.snapshot.network_note">
      网络占用口径：{{ store.snapshot.network_note }}
    </p>
  </div>
</template>

<style scoped>
.ru {
  height: 100vh;
  box-sizing: border-box;
  padding: 10px 12px 12px;
  background: #0c0e14;
  color: #cbd3e1;
  font: 12px/1.5 ui-monospace, Consolas, "Cascadia Mono", monospace;
  display: flex;
  flex-direction: column;
  gap: 8px;
  overflow: hidden;
}
.ru-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.ru-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  color: #e6ebf5;
}
.ru-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #6b7688;
}
.ru-dot.live {
  background: #4bd07a;
  box-shadow: 0 0 8px #4bd07a;
}
.ru-tag,
.ru-kind {
  padding: 1px 6px;
  border: 1px solid #2c3752;
  border-radius: 999px;
  font-size: 10px;
  color: #8aa0c0;
}
.ru-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}
.ru-stamp {
  color: #7f8ea8;
  font-size: 10.5px;
}
.ru-btn {
  background: #161b26;
  color: #cbd3e1;
  border: 1px solid #263044;
  border-radius: 4px;
  padding: 3px 10px;
  font: inherit;
  cursor: pointer;
}
.ru-btn:hover:not(:disabled) {
  border-color: #3d5680;
  color: #fff;
}
.ru-btn:disabled {
  opacity: 0.5;
  cursor: default;
}
.ru-btn.small {
  padding: 2px 12px;
  font-size: 11px;
}
.ru-btn.danger:not(:disabled) {
  border-color: #5c2424;
  color: #ff9a9a;
}
.ru-btn.danger:hover:not(:disabled) {
  border-color: #ff6b6b;
  color: #fff;
}
.ru-err,
.ru-notice {
  margin: 0;
  padding: 6px 8px;
  font-size: 11px;
  white-space: pre-wrap;
}
.ru-err {
  border-left: 2px solid #ff6b6b;
  background: #1d1216;
  color: #ff9a9a;
}
.ru-notice {
  border-left: 2px solid #4bd07a;
  background: #10231a;
  color: #4bd07a;
}
.ru-summary {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 8px;
}
.ru-stat {
  border: 1px solid #1e2634;
  border-radius: 6px;
  background: #111621;
  padding: 8px 10px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.ru-stat-label {
  color: #7f8ea8;
  font-size: 10.5px;
}
.ru-stat-value {
  color: #e6ebf5;
  font-size: 18px;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.ru-stat-value.on {
  color: #4bd07a;
}
.ru-stat-value small {
  font-size: 11px;
  font-weight: 400;
  color: #7f8ea8;
}
.ru-stat-sub {
  color: #6b7688;
  font-size: 10px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.ru-cards {
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 8px;
  align-content: start;
  padding-right: 4px;
}
.ru-card {
  border: 1px solid #1e2634;
  border-radius: 8px;
  background: #111621;
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.ru-card.offline {
  opacity: 0.78;
}
.ru-card.owned {
  border-color: #245c39;
}
.ru-card-head {
  display: flex;
  align-items: center;
  gap: 8px;
}
.ru-icon {
  font-size: 16px;
}
.ru-card-name {
  color: #e6ebf5;
  font-size: 12.5px;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ru-badge {
  margin-left: auto;
  font-size: 10px;
  padding: 0 7px;
  border-radius: 999px;
  border: 1px solid #2c3752;
  white-space: nowrap;
}
.ru-badge.on {
  color: #4bd07a;
  border-color: #245c39;
}
.ru-badge.off {
  color: #ff9a9a;
  border-color: #5c2424;
}
.ru-metrics {
  display: grid;
  grid-template-columns: repeat(5, 1fr);
  gap: 6px;
}
.ru-metric {
  background: #0f131c;
  border: 1px solid #1a2130;
  border-radius: 5px;
  padding: 4px 6px;
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}
.ru-metric span {
  color: #7f8ea8;
  font-size: 10px;
}
.ru-metric b {
  color: #dfe7f5;
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.ru-metric b.dim {
  color: #5d6a7e;
}
.ru-note {
  margin: 0;
  color: #6b7688;
  font-size: 10px;
  line-height: 1.45;
}
.ru-card-foot {
  display: flex;
  gap: 8px;
  align-items: center;
}
.ru-ext-hint {
  color: #5d6a7e;
  font-size: 10px;
  font-style: italic;
}
.ru-foot {
  margin: 0;
  color: #6b7688;
  font-size: 10px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
