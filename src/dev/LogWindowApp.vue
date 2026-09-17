<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

/** tail_dev_log 命令返回（对应 Rust logger::TailResult） */
interface TailResult {
  lines: string[];
  next_offset: number;
  reset: boolean;
  truncated: boolean;
  file_len: number;
  path: string;
}

/** 解析后的一行日志：`[时间] [级别] 内容` */
interface ParsedLine {
  id: number;
  ts: string;
  level: string;
  text: string;
  raw: string;
}

/** DOM 行数上限：超出从头部丢弃，防止长开窗口内存/DOM 膨胀 */
const MAX_LINES = 2000;
const POLL_MS = 500;

const lines = ref<ParsedLine[]>([]);
const nextOffset = ref(0);
const filePath = ref("");
const fileLen = ref(0);
const paused = ref(false);
const filterText = ref("");
const rotationNote = ref("");
const errorMsg = ref("");
/** 视口是否贴底（贴底=自动跟随新日志） */
const atBottom = ref(true);
/** 暂停跟随时累计的新增行数 */
const newCount = ref(0);

let timer: number | null = null;
let inFlight = false;
let lineSeq = 0;
const bodyEl = ref<HTMLElement | null>(null);

const LINE_RE = /^\[([^\]]+)\]\s*(?:\[([A-Z:._-]+)\]\s*)?(.*)$/;

function parseLine(raw: string): ParsedLine {
  const m = LINE_RE.exec(raw);
  const fullTs = m?.[1] ?? "";
  // 只展示时分秒.毫秒，完整时间戳放 title
  const shortTs = fullTs.includes(" ") ? fullTs.slice(fullTs.indexOf(" ") + 1) : fullTs;
  return {
    id: lineSeq++,
    ts: shortTs,
    level: m?.[2] ?? "LOG",
    text: m?.[3] ?? raw,
    raw,
  };
}

const LEVEL_CLASS: Record<string, string> = {
  "AOP:CALL": "lv-call",
  "AOP:RET": "lv-ret",
  "AOP:ERR": "lv-err",
  INFO: "lv-info",
  LOGGER: "lv-logger",
};

function levelClass(lv: string): string {
  return LEVEL_CLASS[lv] ?? "lv-log";
}

const filtered = computed(() => {
  const kw = filterText.value.trim().toLowerCase();
  if (!kw) return lines.value;
  return lines.value.filter((l) => l.raw.toLowerCase().includes(kw));
});

const fileLenText = computed(() => {
  const mb = fileLen.value / (1024 * 1024);
  return mb >= 1 ? `${mb.toFixed(2)} MB` : `${(fileLen.value / 1024).toFixed(1)} KB`;
});

/** 运行态构建信息：一眼分辨 dev（vite 实时源）还是打包（dist 静态资源，可能是旧构建） */
interface AppInfo {
  version: string;
  dev: boolean;
  exe: string;
  exe_mtime_unix: number | null;
}

const buildText = ref("");
const buildTitle = ref("");

async function loadAppInfo() {
  try {
    const info = await invoke<AppInfo>("app_info");
    const t = info.exe_mtime_unix ? new Date(info.exe_mtime_unix * 1000) : null;
    const stamp = t
      ? `${String(t.getMonth() + 1).padStart(2, "0")}-${String(t.getDate()).padStart(2, "0")} ${String(t.getHours()).padStart(2, "0")}:${String(t.getMinutes()).padStart(2, "0")}`
      : "?";
    buildText.value = `v${info.version} ${info.dev ? "dev" : "打包"} · exe ${stamp}`;
    buildTitle.value = `可执行文件：${info.exe}\n前端来源：${info.dev ? "vite 开发服务器（实时源码）" : "dist 静态资源（需 npm run build 更新）"}`;
  } catch {
    buildText.value = "";
  }
}

/** 诊断读数列（BUG-2026-0910-001）：轮询耗时/首帧耗时直接上状态栏，卡不卡看数字 */
const lastPollMs = ref(0);
const firstPollMs = ref(0);

function isNearBottom(el: HTMLElement): boolean {
  return el.scrollHeight - el.scrollTop - el.clientHeight < 40;
}

async function scrollToBottom() {
  newCount.value = 0;
  atBottom.value = true;
  await nextTick();
  if (bodyEl.value) bodyEl.value.scrollTop = bodyEl.value.scrollHeight;
}

function onScroll() {
  if (!bodyEl.value) return;
  const bottom = isNearBottom(bodyEl.value);
  if (bottom && !atBottom.value) {
    atBottom.value = true;
    newCount.value = 0;
  } else if (!bottom) {
    atBottom.value = false;
  }
}

function pushLines(newLines: string[]) {
  if (!newLines.length) return;
  // 洪峰护栏（BUG-2026-0910-001）：单次最多渲染最新 200 行——终端语义丢最旧，
  // 把扫描期洪峰（每秒上千行）的 DOM 改造量压到 ~400 行/秒，避免副窗口
  // 渲染器跑满后与主窗口互抢（共用 WebView2 GPU/浏览器进程）
  const MAX_ROWS_PER_FLUSH = 200;
  if (newLines.length > MAX_ROWS_PER_FLUSH) newLines = newLines.slice(-MAX_ROWS_PER_FLUSH);
  const parsed = newLines.map(parseLine);
  lines.value.push(...parsed);
  if (lines.value.length > MAX_LINES) {
    lines.value.splice(0, lines.value.length - MAX_LINES);
  }
  if (atBottom.value) {
    void scrollToBottom();
  } else {
    newCount.value += parsed.length;
  }
}

async function poll() {
  // 防重入：上一轮 invoke 未返回时跳过本轮，避免卡顿时 IPC 请求积压放大卡顿
  if (paused.value || document.hidden || inFlight) return;
  inFlight = true;
  const t0 = performance.now();
  try {
    const r = await invoke<TailResult>("tail_dev_log", { offset: nextOffset.value });
    const cost = performance.now() - t0;
    lastPollMs.value = Math.round(cost);
    if (!firstPollMs.value) firstPollMs.value = Math.round(cost);
    errorMsg.value = "";
    if (r.path) filePath.value = r.path;
    fileLen.value = r.file_len;
    if (r.reset) {
      // 日志被清理线程清空/轮转：清屏从头读
      lines.value = [];
      rotationNote.value = "日志文件已轮转/清空，已重新加载";
      window.setTimeout(() => (rotationNote.value = ""), 4000);
    }
    if (r.truncated) {
      rotationNote.value = "仅显示最近 64KB 日志";
      window.setTimeout(() => (rotationNote.value = ""), 4000);
    }
    nextOffset.value = r.next_offset;
    pushLines(r.lines);
  } catch (e) {
    errorMsg.value = String(e);
  } finally {
    inFlight = false;
  }
}

function clearView() {
  lines.value = [];
  newCount.value = 0;
}

onMounted(() => {
  void loadAppInfo();
  void poll();
  timer = window.setInterval(() => void poll(), POLL_MS);
});

onUnmounted(() => {
  if (timer !== null) window.clearInterval(timer);
  timer = null;
});
</script>

<template>
  <div class="logwin">
    <header class="lw-bar">
      <span class="lw-dots" aria-hidden="true"><i /><i /><i /></span>
      <span class="lw-title">开发者视角 · 实时日志</span>
      <input
        v-model="filterText"
        class="lw-filter"
        type="text"
        placeholder="过滤…"
        spellcheck="false"
      />
      <button class="lw-btn" type="button" @click="paused = !paused">
        {{ paused ? "▶ 恢复" : "⏸ 暂停" }}
      </button>
      <button class="lw-btn" type="button" @click="clearView">清屏</button>
      <span class="lw-status" :title="filePath">
        {{ lines.length }} 行 · {{ fileLenText }} · 轮询 {{ lastPollMs }}ms<template v-if="firstPollMs">
          （首帧 {{ firstPollMs }}ms）</template
        ><template v-if="paused"> · 已暂停</template>
      </span>
      <span v-if="buildText" class="lw-build" :title="buildTitle">{{ buildText }}</span>
    </header>

    <p v-if="rotationNote" class="lw-note">{{ rotationNote }}</p>
    <p v-if="errorMsg" class="lw-note lw-note-err">{{ errorMsg }}</p>

    <div ref="bodyEl" class="lw-body" @scroll="onScroll">
      <div v-for="l in filtered" :key="l.id" class="lw-line" :title="l.raw">
        <span class="lw-ts">{{ l.ts }}</span>
        <span class="lw-lv" :class="levelClass(l.level)">{{ l.level }}</span>
        <span class="lw-text">{{ l.text }}</span>
      </div>
      <div v-if="!filtered.length" class="lw-empty">（暂无日志{{ filterText ? " 匹配" : "" }}）</div>
    </div>

    <button v-show="!atBottom" class="lw-jump" type="button" @click="scrollToBottom">
      ↓ 回到底部<template v-if="newCount > 0">（{{ newCount }} 条新日志）</template>
    </button>
  </div>
</template>

<style scoped>
.logwin {
  position: fixed;
  inset: 0;
  display: flex;
  flex-direction: column;
  background: #0c0e14;
  color: #d3dae8;
  font-family: var(--font-mono);
}

.lw-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  background: #141824;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  flex-shrink: 0;
}

.lw-dots {
  display: inline-flex;
  gap: 5px;
  margin-right: 2px;
}
.lw-dots i {
  width: 10px;
  height: 10px;
  border-radius: 50%;
}
.lw-dots i:nth-child(1) {
  background: #ff5f57;
}
.lw-dots i:nth-child(2) {
  background: #febc2e;
}
.lw-dots i:nth-child(3) {
  background: #28c840;
}

.lw-title {
  font-size: 12.5px;
  font-weight: 600;
  color: #aeb8cc;
  margin-right: 8px;
  white-space: nowrap;
}

.lw-filter {
  width: 160px;
  height: 26px;
  padding: 0 10px;
  font-size: 12px;
  font-family: var(--font-mono);
  color: #d3dae8;
  background: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 6px;
  outline: none;
}
.lw-filter:focus {
  border-color: #4f7cf0;
}

.lw-btn {
  height: 26px;
  padding: 0 10px;
  font-size: 12px;
  font-family: var(--font-mono);
  color: #c4cddd;
  background: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 6px;
  cursor: pointer;
  white-space: nowrap;
}
.lw-btn:hover {
  background: rgba(255, 255, 255, 0.12);
}

.lw-status {
  margin-left: auto;
  font-size: 11.5px;
  color: #6e7890;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 46%;
}

/* 构建指纹：dev/打包 + exe 构建时间，防止再跑错包（BUG-2026-0910-001 同类困惑） */
.lw-build {
  font-size: 11px;
  color: #55607a;
  white-space: nowrap;
  padding-left: 8px;
  border-left: 1px solid rgba(255, 255, 255, 0.1);
  flex-shrink: 0;
}

.lw-note {
  margin: 0;
  padding: 4px 12px;
  font-size: 11.5px;
  color: #e8c876;
  background: rgba(232, 200, 118, 0.08);
  border-bottom: 1px solid rgba(232, 200, 118, 0.15);
  flex-shrink: 0;
}
.lw-note-err {
  color: #ff8585;
  background: rgba(255, 107, 107, 0.08);
  border-bottom-color: rgba(255, 107, 107, 0.2);
}

.lw-body {
  flex: 1;
  overflow-y: auto;
  padding: 8px 0;
  font-size: 12px;
  line-height: 1.65;
}

.lw-line {
  display: flex;
  gap: 10px;
  padding: 0 12px;
  white-space: pre-wrap;
  word-break: break-all;
}
.lw-line:hover {
  background: rgba(255, 255, 255, 0.04);
}

.lw-ts {
  color: #5d6678;
  flex-shrink: 0;
}

.lw-lv {
  flex-shrink: 0;
  min-width: 68px;
  font-weight: 600;
}
.lv-call {
  color: #8b93a7;
}
.lv-ret {
  color: #6cb2ff;
}
.lv-err {
  color: #ff6b6b;
}
.lv-info {
  color: #7ee29a;
}
.lv-logger {
  color: #e8c876;
}
.lv-log {
  color: #9aa5bb;
}

.lw-text {
  color: #c7d0e2;
}

.lw-empty {
  padding: 24px 12px;
  text-align: center;
  color: #5d6678;
}

.lw-jump {
  position: absolute;
  right: 18px;
  bottom: 18px;
  padding: 6px 14px;
  font-size: 12px;
  font-family: var(--font-mono);
  color: #e6ecf8;
  background: #2c3540;
  border: 1px solid rgba(255, 255, 255, 0.16);
  border-radius: 999px;
  cursor: pointer;
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.45);
}
.lw-jump:hover {
  background: #38424f;
}

.lw-body::-webkit-scrollbar {
  width: 10px;
}
.lw-body::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.14);
  border-radius: 5px;
}
.lw-body::-webkit-scrollbar-track {
  background: transparent;
}
</style>
