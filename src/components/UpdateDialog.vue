<script setup lang="ts">
/**
 * 检查更新弹窗（FEAT-212）
 *
 * 由 `views/Home.vue` 的设置菜单打开，渲染时 teleport 到 body
 * （页头 `.user-box` 带 backdrop-filter，会把它变成 position:fixed 的包含块）。
 *
 * 设计取舍：
 *  - 组件只渲染 store 的状态，不自己拼状态：下载/安装要能活过「关掉这个弹窗」，
 *    状态放组件里会在卸载时丢掉进度；
 *  - 状态行按「正在检查 → 出错 → 有更新 → 已是最新 → 未检查」排优先级，
 *    同一时刻只显示一条结论（并列显示会让人不知道到底怎么了）；
 *  - 安装是**退出应用**的动作，必须先确认；安装包与校验都已完成，退出是预期行为。
 */
import { computed, onBeforeUnmount, onMounted } from "vue";
import { useUpdateStore } from "../stores/update";
import { useThemeStore } from "../stores/theme";
import { notify } from "../composables/useNotify";
import { formatBytes } from "../utils/bytes";
import {
  formatCheckedAt,
  formatEta,
  formatPublishedAt,
  formatSpeed,
} from "../utils/updateFormat";

const emit = defineEmits<{ close: [] }>();
const update = useUpdateStore();
const theme = useThemeStore();

/** 可下载的安装包（只有「确实有更新」时才认，避免拿旧 info 的资产误导用户） */
const asset = computed(() => (update.info?.has_update ? update.info.asset : null));
/** 上次检查时间（`checked_at_ms` 是 epoch 毫秒；0 → 未检查） */
const checkedAt = computed(() => formatCheckedAt(update.info?.checked_at_ms ?? 0));
/** 发布时间（空串时整段不渲染） */
const publishedAt = computed(() => formatPublishedAt(update.info?.published_at ?? ""));

/** 进度百分比（总长未知时为 0：测速阶段后端还没给出 total） */
const percent = computed(() => {
  const d = update.download;
  if (!d.total || d.total <= 0) return 0;
  return Math.min(100, Math.round((d.received / d.total) * 100));
});

/** 安装包文件名（只用于「已就绪（xxx）」里的 xxx，取不到就退回固定文案） */
const installerName = computed(() => {
  const p = update.download.path ?? "";
  if (!p) return "";
  const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
  return i >= 0 ? p.slice(i + 1) : p;
});

/**
 * 下载按钮文案。
 *
 * `asset.size === 0` 是回退通道（tag 页拼资产）拿不到体积的情况，此时不能拼体积：
 * 体积格式化对 0 固定输出「0 KB」，「下载更新（0 KB）」会让用户以为安装包是空的。
 */
const downloadLabel = computed(() => {
  const size = asset.value?.size ?? 0;
  return size > 0 ? `下载更新（${formatBytes(size)}）` : "下载更新";
});

/** 下载阶段的说明文字（running 时进度条上方那行） */
const stageText = computed(() => {
  switch (update.download.stage) {
    case "probing":
      return "正在测试下载通道…";
    case "verifying":
      return "正在校验安装包…";
    default:
      return "正在下载…";
  }
});

/* Esc 关闭由本组件自持（capture 监听 + 阻止冒泡）：
   Home 的 Esc 处理器负责它自己的另两个弹窗，这里不重复接管，也不让它被连带关闭。 */
function onKey(e: KeyboardEvent) {
  if (e.key !== "Escape") return;
  e.preventDefault();
  e.stopPropagation();
  emit("close");
}
onMounted(() => document.addEventListener("keydown", onKey, true));
onBeforeUnmount(() => document.removeEventListener("keydown", onKey, true));

/** 立即安装：先确认（安装会退出本程序），再交给 store */
async function onInstall() {
  const ok = await notify.confirm(
    "安装并退出应用？",
    "安装程序会自动启动，本应用随后退出；相册与模型数据不会丢失。",
    { confirmText: "安装", type: "primary" },
  );
  if (ok) await update.install();
}
</script>

<template>
  <teleport to="body">
    <div class="pm-modal" @click.self="emit('close')">
      <div
        class="pm-dialog ud-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="update-title"
        :style="theme.dialogVarStyle"
      >
        <div class="ud-head">
          <h3 id="update-title">🔄 检查更新</h3>
          <button class="ud-x" type="button" aria-label="关闭更新窗口" @click="emit('close')">
            ✕
          </button>
        </div>

        <!-- 当前版本 + 上次检查时间 -->
        <div class="ud-version">
          <span class="ud-current">v{{ update.current || "…" }}</span>
          <span class="ud-checked">上次检查：{{ checkedAt }}</span>
        </div>

        <!-- 状态行：同一时刻只给一条结论 -->
        <p v-if="update.checking" class="ud-status">正在检查…</p>
        <p v-else-if="update.checkError" class="ud-status ud-error">
          {{ update.checkError }}
          <button class="ud-link" type="button" @click="update.check(true)">重试</button>
        </p>
        <p v-else-if="update.info?.has_update" class="ud-status ud-status-new">
          发现新版本 <b>v{{ update.info?.latest }}</b>
          <span v-if="publishedAt" class="ud-published">发布于 {{ publishedAt }}</span>
        </p>
        <p v-else-if="update.info" class="ud-status">已是最新版本</p>
        <p v-else class="ud-status">点「检查更新」获取最新版本</p>

        <!-- 更新说明：Release body 是纯文本，用 pre-wrap 保留段落 -->
        <div v-if="update.info?.has_update && update.info?.notes" class="ud-notes">
          <span class="ud-notes-title">更新说明</span>
          <div class="ud-notes-body">{{ update.info?.notes }}</div>
        </div>

        <!-- 下载区：只有「有更新且拿到安装包资产」才出现 -->
        <div v-if="update.hasUpdate && asset" class="ud-download">
          <!-- 未开始 / 已取消 / 出错 -->
          <template v-if="!update.download.running && update.download.stage !== 'done'">
            <p v-if="update.download.stage === 'error'" class="ud-error">
              {{ update.download.error || "下载失败，请重试" }}
            </p>
            <button
              class="pm-btn pm-btn-primary ud-main-btn"
              type="button"
              :disabled="update.download.stage === 'probing'"
              @click="update.startDownload()"
            >
              {{ update.download.stage === "cancelled" ? "继续下载" : downloadLabel }}
            </button>
          </template>

          <!-- 下载中 -->
          <template v-else-if="update.download.running">
            <div class="ud-progress-head">
              <span>{{ stageText }}</span>
              <span class="ud-pct">{{ percent }}%</span>
            </div>
            <div
              class="ud-bar"
              role="progressbar"
              aria-valuemin="0"
              aria-valuemax="100"
              :aria-valuenow="percent"
              :aria-label="`下载进度 ${percent}%`"
            >
              <div class="ud-bar-fill" :style="{ width: percent + '%' }"></div>
            </div>
            <div v-if="update.download.total > 0" class="ud-meta">
              <span>{{ formatBytes(update.download.received) }} / {{ formatBytes(update.download.total) }}</span>
              <span v-if="formatSpeed(update.download.bytes_per_sec)">
                {{ formatSpeed(update.download.bytes_per_sec) }}
              </span>
              <span v-if="formatEta(update.download.received, update.download.total, update.download.bytes_per_sec)">
                {{ formatEta(update.download.received, update.download.total, update.download.bytes_per_sec) }}
              </span>
            </div>
            <button class="pm-btn ud-cancel-btn" type="button" @click="update.cancelDownload()">
              取消
            </button>
          </template>

          <!-- 已就绪 -->
          <template v-else>
            <p class="ud-ready">安装包已就绪（{{ installerName || asset?.name || "安装包" }}）</p>
            <div class="ud-actions">
              <button class="pm-btn" type="button" @click="update.openFolder()">打开下载文件夹</button>
              <button
                class="pm-btn pm-btn-primary"
                type="button"
                :disabled="!update.readyToInstall"
                @click="onInstall"
              >
                立即安装
              </button>
            </div>
          </template>
        </div>

        <!-- 底部：检查入口 + 关闭 -->
        <div class="ud-foot">
          <p class="ud-note">应用启动后会每 6 小时静默检查一次；安装会退出本程序。</p>
          <p class="ud-note">安装包来自 GitHub Releases · howforever2000/PhotoMangementSys</p>
          <div class="ud-foot-actions">
            <button
              class="pm-btn"
              type="button"
              :disabled="update.checking || update.download.running"
              @click="update.check(true)"
            >
              检查更新
            </button>
            <button class="pm-btn pm-btn-primary" type="button" @click="emit('close')">关闭</button>
          </div>
        </div>
      </div>
    </div>
  </teleport>
</template>

<style scoped>
/* 比通用 .pm-dialog 宽一点：进度条 + 三列元信息需要横向空间 */
.ud-dialog {
  width: min(520px, 92vw);
}

.ud-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 12px;
}

.ud-head h3 {
  margin: 0;
  font-size: 18px;
  font-weight: 600;
  color: var(--pm-text);
}

.ud-x {
  width: 28px;
  height: 28px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: var(--pm-hint);
  font-size: 16px;
  line-height: 1;
  cursor: pointer;
  transition: background var(--dur-2), color var(--dur-2);
}

.ud-x:hover {
  background: var(--pm-btn-hover);
  color: var(--pm-text);
}

/* 当前版本行：左侧版本号是主信息，右侧上次检查时间只做参考 */
.ud-version {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  padding: 8px 10px;
  margin-bottom: 10px;
  background: var(--pm-input-bg);
  border: 1px solid var(--pm-soft-border);
  border-radius: 10px;
}

.ud-current {
  font-size: 14px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  color: var(--pm-text);
}

.ud-checked {
  font-size: 11.5px;
  color: var(--pm-hint);
}

.ud-status {
  margin: 0 0 10px;
  font-size: 13.5px;
  line-height: 1.6;
  color: var(--pm-label);
}

/* 发现新版本：这是本弹窗唯一想让人看见的一句，用正文色 + 加粗压过周围 */
.ud-status-new {
  color: var(--pm-text);
}

.ud-status-new b {
  font-weight: 700;
}

.ud-published {
  margin-left: 8px;
  font-size: 11.5px;
  font-weight: 400;
  color: var(--pm-hint);
}

.ud-error {
  color: var(--color-danger);
}

/* 重试做成行内链接：它是错误文案的附属动作，不该占一个主按钮的位置 */
.ud-link {
  margin-left: 6px;
  padding: 0;
  font-size: 13px;
  color: var(--color-primary);
  background: none;
  border: 0;
  text-decoration: underline;
  cursor: pointer;
}

.ud-notes {
  margin-bottom: 12px;
}

.ud-notes-title {
  display: block;
  margin-bottom: 6px;
  font-size: 12.5px;
  font-weight: 600;
  color: var(--pm-label);
}

/* 更新说明可能很长：给固定可视高度 + 滚动，避免把按钮顶出屏幕 */
.ud-notes-body {
  max-height: 180px;
  overflow: auto;
  padding: 10px 12px;
  font-size: 13px;
  line-height: 1.65;
  white-space: pre-wrap;
  color: var(--pm-label);
  background: var(--pm-input-bg);
  border: 1px solid var(--pm-soft-border);
  border-radius: 10px;
}

.ud-download {
  padding: 12px;
  margin-bottom: 12px;
  background: var(--pm-input-bg);
  border: 1px solid var(--pm-soft-border);
  border-radius: 12px;
}

.ud-main-btn {
  width: 100%;
}

.ud-progress-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 8px;
  font-size: 12.5px;
  color: var(--pm-label);
}

.ud-pct {
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  color: var(--pm-text);
}

/* 进度条轨道用软底色，填充用主题强调色（深浅模式下都能看清边缘） */
.ud-bar {
  height: 8px;
  overflow: hidden;
  background: var(--pm-btn-bg);
  border: 1px solid var(--pm-soft-border);
  border-radius: 999px;
}

.ud-bar-fill {
  height: 100%;
  background: var(--color-primary);
  border-radius: inherit;
  transition: width 200ms ease;
}

.ud-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  margin-top: 8px;
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
  color: var(--pm-hint);
}

.ud-cancel-btn {
  margin-top: 10px;
}

.ud-ready {
  margin: 0 0 10px;
  font-size: 13px;
  font-weight: 600;
  color: var(--color-ok-text);
}

.ud-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
}

.ud-foot {
  margin-top: 4px;
  padding-top: 12px;
  border-top: 1px solid var(--pm-soft-border);
}

.ud-note {
  margin: 0 0 10px;
  font-size: 11.5px;
  line-height: 1.6;
  color: var(--pm-hint);
}

/* 来源说明紧跟上一行：两条小字要读成一个注脚块，不能拉开 10px 的空当 */
.ud-note + .ud-note {
  margin-top: -4px;
}

.ud-foot-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
}

/* 禁用态：降透明度 + 去指针，避免「点了没反应」的困惑 */
.ud-dialog button:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* 焦点可见：弹窗里全是按钮，键盘用户必须能看出当前落在哪一个 */
.ud-dialog button:focus-visible,
.ud-dialog .ud-link:focus-visible {
  outline: 2px solid var(--color-primary);
  outline-offset: 2px;
}
</style>
