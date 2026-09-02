<script setup lang="ts">
/**
 * 批量导入失败详情对话框（FEAT-034-A）
 *
 * 用途：批量导入相册时，弹窗展示每个失败项的「文件夹名 + 原因」明细，
 * 并按错误类型给出解决建议、提示用户提高下次导入成功率。
 *
 * 设计要点：
 * 1. 错误明细按行展示，每行可单独复制/重试该目录（重试后会自动从列表移除成功的项）。
 * 2. 顶部提供「全部重试」按钮：仅重试 error 列表中仍存在的目录。
 * 3. 错误类型分类（基于 `result.errors[i]` 的 `folder: msg` 文本特征）：
 *    - 路径不存在（PathNotExist/目录不存在）→ 文件被移动/删除，建议「重新选择根目录」
 *    - UNIQUE 冲突（同名已存在）→ 提示「该路径已属于当前用户的某个相册」
 *    - 权限（Permission/拒绝访问）→ 提示「右键以管理员身份运行 / 检查目录权限」
 *    - 路径非法（特殊字符/过长）→ 提示「避免使用 \\ / : * ? " < > | 等字符」
 *    - 其他 → 显示原始错误
 * 4. 错误明细中可能包含中文逗号/换行，已做行级解析兜底。
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useThemeStore } from "../stores/theme";
import type { ImportResult } from "../stores/album";

interface ParsedError {
  folder: string;
  message: string;
  /** 错误归类：用于给出对应解决建议 */
  category: "path_missing" | "duplicate" | "permission" | "invalid_path" | "unknown";
  /** 分类对应建议文案 */
  advice: string;
}

const props = defineProps<{
  visible: boolean;
  /** 原始 ImportResult（含 imported/skipped/errors） */
  result: ImportResult | null;
  /** 被重试的目录路径（被外部回调消费后由调用方 set 进来） */
  retriedPaths?: string[];
}>();

/** FEAT-034-C：是否有相对「已存在」的跳过项（跨/同用户占用）——用于友好展示 */
const existsItems = computed(() => props.result?.skipped_conflicts ?? []);
/** 是否仅有「已存在」（无真实失败）——此时弹窗应为信息态而非报错态 */
const onlyExists = computed(() => (props.result?.errors?.length ?? 0) === 0 && existsItems.value.length > 0);
const dialogTitle = computed(() =>
  onlyExists.value
    ? `📂 ${existsItems.value.length} 个相册已导入`
    : "⚠️ 批量导入未全部完成",
);

const emit = defineEmits<{
  (e: "close"): void;
  /** 「重试单个失败项」或「全部重试」：把该目录路径传回给调用方 */
  (e: "retry", paths: string[]): void;
  /** 「重新选根目录」按钮：让调用方打开文件夹选择器 */
  (e: "reimport"): void;
}>();

const theme = useThemeStore();

/* ---- 错误解析 ---- */

/**
 * 解析 `result.errors[i]` 文本（后端格式：`{folder_name}: {err}`），
 * 得到 ParsedError[]。后端在 lib.rs:2009 用 `format!("{folder_name}: {e}")` 拼装，
 * 所以首个英文冒号（或中文冒号）即为分隔。文件夹名中可能不含冒号。
 */
const parsedErrors = computed<ParsedError[]>(() => {
  const list = props.result?.errors ?? [];
  const out: ParsedError[] = [];
  for (const raw of list) {
    if (!raw) continue;
    // 兼容中英文冒号：取首次出现作为分隔
    const idxEn = raw.indexOf(": ");
    const idxCn = raw.indexOf("：");
    let folder = raw;
    let message = "";
    if (idxEn >= 0 && (idxCn < 0 || idxEn < idxCn)) {
      folder = raw.slice(0, idxEn).trim();
      message = raw.slice(idxEn + 2).trim();
    } else if (idxCn >= 0) {
      folder = raw.slice(0, idxCn).trim();
      message = raw.slice(idxCn + 1).trim();
    } else {
      // 兜底：拿不到分隔就当 unknown
      message = raw;
    }
    out.push(classify(folder, message));
  }
  return out;
});

/** 错误归类：根据文案特征匹配并给出解决建议 */
function classify(folder: string, message: string): ParsedError {
  const msg = message.toLowerCase();
  const folderLc = folder.toLowerCase();

  if (
    message.includes("路径不存在") ||
    message.includes("不是文件夹") ||
    msg.includes("pathnotexist") ||
    msg.includes("notfound") ||
    msg.includes("os error 2")
  ) {
    return {
      folder,
      message,
      category: "path_missing",
      advice: "该目录在导入过程中被移动/删除，请检查后重新选择根目录。",
    };
  }
  if (
    message.includes("UNIQUE") ||
    message.includes("已存在") ||
    message.includes("constraint") ||
    folderLc.startsWith("duplicate")
  ) {
    return {
      folder,
      message,
      category: "duplicate",
      advice: "该路径已属于当前用户的某个相册（同名相册）。可前往「相册列表」确认或删除旧条目后重试。",
    };
  }
  if (
    message.includes("权限") ||
    message.includes("Permission") ||
    message.includes("Access") ||
    msg.includes("os error 5") ||
    msg.includes("os error 13")
  ) {
    return {
      folder,
      message,
      category: "permission",
      advice: "无读权限。请右键以管理员身份运行本软件，或在系统文件管理器中检查该目录的读取权限。",
    };
  }
  if (
    message.includes("非法") ||
    message.includes("invalid") ||
    message.includes("路径") ||
    message.length === 0
  ) {
    return {
      folder,
      message,
      category: "invalid_path",
      advice: "路径可能含特殊字符 / 过长。建议把目标目录放到纯英文/数字路径下重试。",
    };
  }
  return {
    folder,
    message,
    category: "unknown",
    advice: "未知错误，建议查看下方「原始错误」或在开发者工具 Console 查看更多上下文。",
  };
}

/* ---- 统计 ---- */
const counts = computed(() => {
  const list = parsedErrors.value;
  return {
    total: list.length,
    path_missing: list.filter((e) => e.category === "path_missing").length,
    duplicate: list.filter((e) => e.category === "duplicate").length,
    permission: list.filter((e) => e.category === "permission").length,
    invalid_path: list.filter((e) => e.category === "invalid_path").length,
    unknown: list.filter((e) => e.category === "unknown").length,
  };
});

/** 解析后的「重试候选」：把 folder 名映射为被跳过的子目录路径（父根目录 + 文件夹名）。
 *  注意：当前 ImportResult 没有保留原始路径，只有文件夹名，
 *  因此这里我们把 folder 名作为路径片段返回；调用方在重试时按需处理。 */
const retryCandidates = computed(() => parsedErrors.value.map((e) => e.folder));

/* ---- 交互 ---- */
function close() {
  emit("close");
}

function retryAll() {
  emit("retry", retryCandidates.value);
}

function retryOne(folder: string) {
  emit("retry", [folder]);
}

function reimport() {
  emit("reimport");
}

/** 复制单条错误到剪贴板（便于贴到 issue / 工单） */
async function copyOne(e: ParsedError) {
  const text = `${e.folder}: ${e.message}`;
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // 旧 WebView 可能无 clipboard：降级用 textarea execCommand
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    try { document.execCommand("copy"); } catch { /* ignore */ }
    document.body.removeChild(ta);
  }
}

/* ---- 主题 & 弹层行为 ---- */
const panelStyle = computed(() => ({
  background: theme.isDark ? "rgba(30,34,46,.96)" : "#fff",
  border: `1px solid ${theme.isDark ? "rgba(255,255,255,.09)" : "rgba(0,0,0,.07)"}`,
}));
const subStyle = computed(() => ({
  color: theme.isDark ? "rgba(214,221,240,.72)" : "rgba(60,70,90,.75)",
}));

const categoryColors: Record<ParsedError["category"], string> = {
  path_missing: "#e8a03c",
  duplicate: "#396cd8",
  permission: "#e5484d",
  invalid_path: "#9333ea",
  unknown: "#666",
};

const categoryLabels: Record<ParsedError["category"], string> = {
  path_missing: "路径不存在",
  duplicate: "同名已存在",
  permission: "权限不足",
  invalid_path: "路径非法",
  unknown: "其他错误",
};

/** ESC 关闭 */
function onKey(e: KeyboardEvent) {
  if (e.key === "Escape" && props.visible) {
    e.preventDefault();
    e.stopPropagation();
    close();
  }
}
onMounted(() => document.addEventListener("keydown", onKey));
onBeforeUnmount(() => document.removeEventListener("keydown", onKey));

/** 打开时滚动到列表顶部 */
watch(
  () => props.visible,
  (v) => {
    if (v) nextTick(() => listEl.value?.scrollTo({ top: 0 }));
  },
);

const listEl = ref<HTMLDivElement | null>(null);
</script>

<template>
  <Teleport to="body">
    <Transition name="import-err-fade">
      <div v-if="visible" class="ier-mask" @click.self="close">
        <div class="ier-dialog" :style="panelStyle" role="dialog" aria-modal="true">
          <div class="ier-head">
            <div class="ier-title">{{ dialogTitle }}</div>
            <button class="ier-close" title="关闭 (Esc)" @click="close">×</button>
          </div>

          <!-- 概览 -->
          <div class="ier-summary" :style="subStyle">
            <span class="ier-pill ier-pill-ok">✓ 成功 {{ result?.imported ?? 0 }}</span>
            <span class="ier-pill ier-pill-skip">⏭ 跳过 {{ result?.skipped ?? 0 }}</span>
            <span v-if="counts.total > 0" class="ier-pill ier-pill-fail">✕ 失败 {{ counts.total }}</span>
          </div>

          <!-- 分类统计（仅在有失败时显示） -->
          <div v-if="counts.total > 0" class="ier-bucket" :style="subStyle">
            <span v-if="counts.duplicate > 0">📁 同名已存在 {{ counts.duplicate }}</span>
            <span v-if="counts.path_missing > 0">🚫 路径不存在 {{ counts.path_missing }}</span>
            <span v-if="counts.permission > 0">🔒 权限不足 {{ counts.permission }}</span>
            <span v-if="counts.invalid_path > 0">⚠️ 路径非法 {{ counts.invalid_path }}</span>
            <span v-if="counts.unknown > 0">❓ 其他 {{ counts.unknown }}</span>
          </div>

          <!-- FEAT-034-C：已经导入（跳过冲突）的相册清单，绿色友好展示 -->
          <div v-if="existsItems.length > 0" ref="listEl" class="ier-list ier-list-exists">
            <div v-if="onlyExists" class="ier-exists-head">{{ existsItems.length }} 个相册此前已导入，本次自动跳过：</div>
            <div v-for="(it, i) in existsItems" :key="'ex-'+i" class="ier-item ier-item-exists">
              <header class="ier-item-head">
                <span class="ier-cat ier-cat-exists">✓ 已存在</span>
                <span class="ier-folder" :title="it.folder">📂 {{ it.folder }}</span>
              </header>
              <div class="ier-advice ier-advice-exists">
                💡 该文件夹已作为相册「{{ it.conflict_album }}」存在，无需重复导入。
              </div>
            </div>
          </div>

          <!-- 错误明细列表（仅真实失败） -->
          <div ref="listEl" class="ier-list">
            <div v-if="counts.total === 0 && existsItems.length === 0" class="ier-empty">🎉 没有失败项</div>
            <article
              v-for="(e, i) in parsedErrors"
              :key="i"
              class="ier-item"
            >
              <header class="ier-item-head">
                <span class="ier-cat" :style="{ background: categoryColors[e.category] }">
                  {{ categoryLabels[e.category] }}
                </span>
                <span class="ier-folder" :title="e.folder">📂 {{ e.folder }}</span>
                <button class="ier-copy" title="复制此条" @click="copyOne(e)">复制</button>
                <button class="ier-retry-one" title="单独重试此目录" @click="retryOne(e.folder)">↻ 重试</button>
              </header>
              <div class="ier-msg" :style="subStyle">
                <span class="ier-msg-label">原始错误：</span>{{ e.message || "(空)" }}
              </div>
              <div class="ier-advice" :style="{ color: categoryColors[e.category] }">
                💡 {{ e.advice }}
              </div>
            </article>
          </div>

          <!-- 底部操作 -->
          <footer class="ier-foot">
            <button class="btn btn-cancel" @click="close">{{ onlyExists ? "知道了" : "关闭" }}</button>
            <button
              v-if="counts.total > 0"
              class="btn btn-secondary"
              title="回到导入对话框重新选择根目录"
              @click="reimport"
            >
              📂 重新选择根目录
            </button>
            <button
              v-if="counts.total > 0"
              class="btn btn-primary"
              title="用原根目录重试所有失败项（被跳过的同名/已导入相册不会重复）"
              @click="retryAll"
            >
              ↻ 全部重试（{{ counts.total }}）
            </button>
          </footer>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.ier-mask {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.55);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1100;
}

.ier-dialog {
  width: 640px;
  max-width: calc(100vw - 32px);
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  border-radius: 14px;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.25);
  overflow: hidden;
}

.ier-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid rgba(127, 127, 127, 0.18);
}
.ier-title {
  font-size: 16px;
  font-weight: 700;
  color: inherit;
}
.ier-close {
  font-size: 22px;
  line-height: 1;
  background: transparent;
  border: none;
  cursor: pointer;
  opacity: 0.6;
}
.ier-close:hover { opacity: 1; }

.ier-summary {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  padding: 10px 20px;
  font-size: 13px;
  color: inherit;
}
.ier-pill {
  padding: 3px 10px;
  border-radius: 999px;
  font-weight: 600;
  font-size: 12px;
}
.ier-pill-ok { background: rgba(47, 158, 68, 0.18); color: #2f9e44; }
.ier-pill-skip { background: rgba(57, 108, 216, 0.18); color: #396cd8; }
.ier-pill-fail { background: rgba(229, 72, 77, 0.18); color: #e5484d; }

.ier-bucket {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  padding: 0 20px 8px;
  font-size: 12px;
  opacity: 0.85;
}

.ier-list {
  flex: 1;
  overflow-y: auto;
  padding: 6px 20px 12px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.ier-empty {
  text-align: center;
  padding: 40px 0;
  opacity: 0.7;
  font-size: 14px;
  color: inherit;
}

.ier-item {
  border: 1px solid rgba(127, 127, 127, 0.18);
  border-radius: 10px;
  padding: 10px 12px;
  background: rgba(127, 127, 127, 0.05);
}

/* FEAT-034-C：已导入（跳过冲突）友好展示 */
.ier-list-exists {
  margin-top: 6px;
}
.ier-exists-head {
  font-size: 13px;
  font-weight: 600;
  color: #2f9e44;
  margin-bottom: 6px;
}
body.theme-dark .ier-exists-head {
  color: #6ed27a;
}
.ier-item-exists {
  border-color: rgba(47, 158, 68, 0.35);
  background: rgba(47, 158, 68, 0.07);
}
.ier-cat-exists {
  background: #2f9e44 !important;
}
body.theme-dark .ier-cat-exists {
  background: #2f9e44 !important;
}
.ier-advice-exists {
  color: #2f9e44 !important;
}
body.theme-dark .ier-advice-exists {
  color: #6ed27a !important;
}
.ier-item-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
}
.ier-cat {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 6px;
  color: #fff;
  font-size: 11px;
  font-weight: 600;
  flex-shrink: 0;
}
.ier-folder {
  flex: 1;
  font-size: 13px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ier-copy,
.ier-retry-one {
  background: transparent;
  border: 1px solid rgba(127, 127, 127, 0.4);
  border-radius: 6px;
  padding: 2px 8px;
  font-size: 11px;
  cursor: pointer;
  color: inherit;
  opacity: 0.8;
}
.ier-copy:hover,
.ier-retry-one:hover {
  opacity: 1;
  background: rgba(127, 127, 127, 0.15);
}

.ier-msg {
  font-size: 12.5px;
  line-height: 1.6;
  word-break: break-all;
  font-family: ui-monospace, "Cascadia Code", "Source Code Pro", Consolas, monospace;
}
.ier-msg-label {
  opacity: 0.6;
  font-family: inherit;
  margin-right: 4px;
}
.ier-advice {
  margin-top: 6px;
  font-size: 12px;
  font-weight: 500;
  opacity: 0.95;
}

.ier-foot {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  padding: 12px 20px;
  border-top: 1px solid rgba(127, 127, 127, 0.18);
}

.btn {
  padding: 7px 16px;
  font-size: 13px;
  border-radius: 8px;
  border: 1px solid rgba(127, 127, 127, 0.3);
  cursor: pointer;
  transition: all 0.15s;
}
.btn-cancel { background: transparent; }
.btn-cancel:hover { background: rgba(127, 127, 127, 0.1); }
.btn-secondary {
  background: rgba(57, 108, 216, 0.1);
  border-color: rgba(57, 108, 216, 0.4);
  color: #396cd8;
}
.btn-secondary:hover { background: rgba(57, 108, 216, 0.18); }
.btn-primary {
  background: #396cd8;
  color: #fff;
  border-color: #396cd8;
}
.btn-primary:hover { background: #2f5fc1; }

/* 过渡 */
.import-err-fade-enter-active,
.import-err-fade-leave-active {
  transition: opacity 0.2s ease;
}
.import-err-fade-enter-active .ier-dialog,
.import-err-fade-leave-active .ier-dialog {
  transition: transform 0.2s ease;
}
.import-err-fade-enter-from,
.import-err-fade-leave-to { opacity: 0; }
.import-err-fade-enter-from .ier-dialog,
.import-err-fade-leave-to .ier-dialog { transform: scale(0.95); }
</style>
