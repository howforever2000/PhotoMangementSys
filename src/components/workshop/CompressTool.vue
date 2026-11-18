<script setup lang="ts">
/**
 * 压缩工作区（FEAT-150）
 * ===================================================================
 * 交互取向：**批量工位**，不是编辑器。
 *
 * - 左边不用面板，参数排成一行：这项任务只有 4 个决定（目标大小 / 长边 / 裁切 / 输出到），
 *   做成侧栏会让中间空出 300px 白板，而参数本身并不需要那么多纵向空间。
 * - 结果直接列在按钮下面：压完就能看见「哪张、原多大、现在多大、用了什么质量」，
 *   不需要再点一次"查看结果"。
 * - 目标大小凑不到时**如实标出来**（后端会缩到长边下限仍不达标就返回最小质量结果），
 *   不能显示成"已完成"——用户拿去发邮件才发现超限，那是一次数据可信度损失。
 *
 * 后端契约见 `src-tauri/src/tools.rs::image_compress`（输出永远是副本，不覆盖原图）。
 */
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useNotify } from "../../composables/useNotify";
import { useCompressTaskStore } from "../../stores/compressTask";
import type { CompressedItem } from "../../types/tools";
import { baseName, dirName, joinPath } from "../../utils/pathParts";
import { formatBytes, savedPercent } from "../../utils/bytes";

const props = defineProps<{ paths: string[] }>();

const notify = useNotify();
/** 压缩任务状态提在 store：切走页面任务在后端继续，切回来靠它恢复进度/结果 */
const task = useCompressTaskStore();

/** 压缩方式：按质量（固定 q）/ 按大小（后端搜索质量凑到目标 KB） */
const mode = ref<"quality" | "size">("size");
const quality = ref(82);
const targetKb = ref(300);
/** 长边上限：0 = 不缩放 */
const maxEdge = ref(1920);
/** 居中裁切比例：空 = 保持原比例 */
const cropAspect = ref("");

/** 输出目录：用户没动过就跟着第一张照片走，动过就不再被覆盖 */
const destDir = ref("");
const destTouched = ref(false);
watch(
  () => props.paths,
  (paths) => {
    if (destTouched.value) return;
    const first = paths[0];
    const dir = first ? dirName(first) : "";
    destDir.value = dir ? joinPath(dir, "_compressed") : "";
  },
  { immediate: true },
);

const running = computed(() => task.running);
const snap = computed(() => task.snapshot);

/* 重新进入页面：拉一次后端权威状态；若仍在跑就续轮询，否则读回已完成的结果 */
onMounted(async () => {
  try {
    await task.refresh();
  } catch {
    /* 后端不可达时保持现状 */
  }
  if (task.running) task.startPolling();
});

// 离开页面只停轮询、保留状态（任务在后端继续，回来再续）
onUnmounted(() => task.disposeKeepState());

// 任务从「运行中」收敛到终态时给一次提示（部分结果保留在 snapshot 里）
watch(
  () => task.snapshot.status,
  (st, prev) => {
    if (prev !== "running") return;
    const s = task.snapshot;
    if (st === "done") {
      const pct = savedPercent(s.totalBefore, s.totalAfter);
      notify.success(
        "压缩完成",
        `${s.items.length} 张 · ${formatBytes(s.totalBefore)} → ${formatBytes(s.totalAfter)}` +
          (pct ? `（${pct}）` : "") +
          (s.failed.length ? `，${s.failed.length} 张失败` : ""),
      );
    } else if (st === "failed") {
      notify.error("压缩失败", s.error || "请检查照片是否可读");
    } else if (st === "cancelled") {
      notify.warning("已停止", `已完成 ${s.items.length}/${s.total} 张，保留已产出的部分结果`);
    }
  },
);

const cropOptions = [
  { value: "", label: "保持原比例" },
  { value: "1:1", label: "1 : 1 方图" },
  { value: "4:3", label: "4 : 3 横图" },
  { value: "3:4", label: "3 : 4 竖图" },
  { value: "16:9", label: "16 : 9 宽幅" },
  { value: "9:16", label: "9 : 16 竖屏" },
];
const edgeOptions = [
  { value: 0, label: "不限制" },
  { value: 2560, label: "2560 px" },
  { value: 1920, label: "1920 px" },
  { value: 1280, label: "1280 px" },
  { value: 1080, label: "1080 px" },
  { value: 800, label: "800 px" },
];

const canRun = computed(() => props.paths.length > 0 && !running.value);

/** 目标字节数（仅在"按大小"模式下有意义） */
const targetBytes = computed(() => (mode.value === "size" ? safeTargetKb.value * 1024 : 0));

/** 输入框可能被清空（NaN）或被滚轮改到越界 —— 交给后端前先夹到合法区间 */
const safeTargetKb = computed(() => {
  const v = Number(targetKb.value);
  return Number.isFinite(v) && v >= 20 ? Math.round(v) : 300;
});
const safeQuality = computed(() => {
  const v = Number(quality.value);
  return Number.isFinite(v) ? Math.min(95, Math.max(1, Math.round(v))) : 82;
});

/** 该张是否没凑到目标大小（后端已尽力：质量到底 + 长边缩到下限） */
function unmet(it: CompressedItem): boolean {
  return targetBytes.value > 0 && it.after > targetBytes.value;
}

async function pickDest() {
  const dir = await openDialog({
    directory: true,
    title: "选择压缩输出目录",
    defaultPath: destDir.value || undefined,
  });
  if (typeof dir !== "string") return; // 用户取消
  destDir.value = dir;
  destTouched.value = true;
}

async function run() {
  if (!canRun.value) return;
  if (!destDir.value) {
    notify.warning("还没有输出目录", "先选择压缩结果要放在哪个文件夹");
    return;
  }
  try {
    // 提交为后端进程级任务，立即返回；进度/结果由 store 轮询对齐
    await task.start(props.paths, destDir.value, {
      targetKb: mode.value === "size" ? safeTargetKb.value : null,
      quality: mode.value === "quality" ? safeQuality.value : null,
      maxEdge: Number(maxEdge.value) || 0,
      cropAspect: cropAspect.value || null,
    });
  } catch (e) {
    notify.error("压缩失败", String(e));
  }
}

/** 停止当前后台任务（粒度=单张之间；已产出的部分结果保留） */
async function onStop() {
  await task.cancel();
}

async function openDest() {
  const dir = destDir.value || snap.value.destDir;
  if (!dir) return;
  try {
    await invoke("open_folder", { path: dir });
  } catch (e) {
    notify.error("打不开目录", String(e));
  }
}
</script>

<template>
  <section class="ct">
    <!-- 参数：四项排成一行。前两项是"压缩到什么程度"，后两项是"裁成什么形状 / 放到哪里" -->
    <div class="ct-opts">
      <div class="field">
        <label class="f-label">目标大小</label>
        <div class="f-body">
          <div class="seg" role="group" aria-label="压缩方式">
            <button
              type="button"
              class="seg-btn"
              :class="{ on: mode === 'size' }"
              @click="mode = 'size'"
            >
              指定大小
            </button>
            <button
              type="button"
              class="seg-btn"
              :class="{ on: mode === 'quality' }"
              @click="mode = 'quality'"
            >
              指定质量
            </button>
          </div>
          <template v-if="mode === 'size'">
            <input v-model.number="targetKb" class="inp inp-num" type="number" min="20" step="10" />
            <span class="unit">KB</span>
          </template>
          <template v-else>
            <input v-model.number="quality" class="inp inp-num" type="number" min="1" max="95" />
            <span class="unit">/ 95</span>
          </template>
        </div>
      </div>

      <div class="field">
        <label class="f-label">长边上限</label>
        <select v-model.number="maxEdge" class="sel">
          <option v-for="o in edgeOptions" :key="o.value" :value="o.value">{{ o.label }}</option>
        </select>
      </div>

      <div class="field">
        <label class="f-label">裁切</label>
        <select v-model="cropAspect" class="sel">
          <option v-for="o in cropOptions" :key="o.value" :value="o.value">{{ o.label }}</option>
        </select>
      </div>

      <div class="field field-dest">
        <label class="f-label">输出到</label>
        <div class="f-body">
          <span class="dest-path" :title="destDir || '尚未选择'">{{ destDir || "尚未选择目录" }}</span>
          <button class="btn btn-sm" type="button" @click="pickDest">选择…</button>
        </div>
      </div>
    </div>

    <p class="ct-note">
      输出为 JPEG 副本（透明底铺白、不含拍摄信息），原图不动；同目录重名自动加后缀。
    </p>

    <!-- 运行中：进度条 + 当前张 + 停止（切页任务不会断，回到本页自动续上） -->
    <div v-if="running" class="ct-progress">
      <div class="ct-bar"><i :style="{ width: task.percent + '%' }" /></div>
      <span class="ct-progress-text">
        {{ task.statusText }} · {{ snap.done }}/{{ snap.total }}
        <template v-if="snap.current"> · {{ baseName(snap.current) }}</template>
      </span>
      <button class="btn btn-sm" type="button" @click="onStop">停止</button>
    </div>

    <div class="ct-actions">
      <button v-if="!running" class="run-btn" type="button" :disabled="!canRun" @click="run">
        {{ `压缩 ${paths.length} 张` }}
      </button>
      <button
        v-if="destDir || snap.destDir"
        class="btn btn-sm"
        type="button"
        :disabled="running"
        @click="openDest"
      >
        打开输出目录
      </button>
      <span v-if="paths.length > 30 && !running" class="ct-hint">
        张数较多，按大小压缩每张要试编码几次，可能需要一两分钟
      </span>
    </div>

    <!-- 结果：摘要 + 逐张明细（不折叠，用户要核对自己那几张；停止后仍保留已完成的） -->
    <div v-if="task.hasResult" class="res">
      <div class="res-sum">
        <strong>{{ snap.items.length }} 张完成</strong>
        <span class="res-dim">
          {{ formatBytes(snap.totalBefore) }} → {{ formatBytes(snap.totalAfter) }}
        </span>
        <span v-if="savedPercent(snap.totalBefore, snap.totalAfter)" class="res-save">
          {{ savedPercent(snap.totalBefore, snap.totalAfter) }}
        </span>
        <span v-if="snap.failed.length" class="res-err">{{ snap.failed.length }} 张失败</span>
        <span v-if="snap.cancelled" class="res-err">已停止</span>
      </div>

      <ul class="res-list">
        <li v-for="it in snap.items" :key="it.dst" class="res-row">
          <span class="res-name" :title="it.dst">{{ baseName(it.src) }}</span>
          <span class="res-dim">{{ it.width }} × {{ it.height }}</span>
          <span class="res-size">
            {{ formatBytes(it.before) }} → {{ formatBytes(it.after) }}
          </span>
          <span class="res-q">q{{ it.quality }}</span>
          <span v-if="unmet(it)" class="res-warn" title="已到最低质量与长边下限，仍超出目标大小">
            未达标
          </span>
        </li>
      </ul>

      <ul v-if="snap.failed.length" class="res-list res-list-fail">
        <li v-for="f in snap.failed" :key="f.src" class="res-row">
          <span class="res-name" :title="f.src">{{ baseName(f.src) }}</span>
          <span class="res-err">{{ f.reason }}</span>
        </li>
      </ul>
    </div>
  </section>
</template>

<style scoped>
.ct {
  padding-top: 4px;
}

/* ---------- 参数行 ---------- */
.ct-opts {
  display: grid;
  /* 宽屏下输出目录列限宽：路径框拉成 900px 只会让右侧变空 */
  grid-template-columns: 300px 150px 150px minmax(220px, 460px);
  gap: 12px 18px;
  align-items: end;
  padding: 14px 16px 12px;
  border-radius: var(--r-ui);
  border: 1px solid var(--c-rule);
  background: var(--hover, rgba(120, 120, 130, 0.06));
}

.field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

/* 字段名用全站统一的小标字阶（与分区标同源），不再另造一套 */
.f-label {
  font-size: var(--fs-label);
  letter-spacing: 0.14em;
  color: var(--fg-2);
}

.f-body {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.seg {
  display: inline-flex;
  align-items: center;
  padding: 2px;
  border-radius: 7px;
  box-shadow: inset 0 0 0 1px var(--c-rule);
  flex-shrink: 0;
}

.seg-btn {
  height: 24px;
  padding: 0 9px;
  border: none;
  background: none;
  border-radius: 5px;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  cursor: pointer;
  transition: color var(--dur-2) var(--ease-out), background var(--dur-2) var(--ease-out);
}
.seg-btn:hover {
  color: var(--fg);
}
.seg-btn.on {
  color: var(--fg);
  background: color-mix(in srgb, var(--c-accent) 16%, transparent);
  font-weight: var(--fw-em);
}

.inp,
.sel {
  height: 30px;
  padding: 0 8px;
  border-radius: 7px;
  border: 1px solid var(--c-rule);
  background: transparent;
  color: var(--fg);
  font-size: var(--fs-body);
  font-variant-numeric: tabular-nums;
  outline: none;
  transition: border-color var(--dur-2) var(--ease-out);
}
.inp:focus,
.sel:focus {
  border-color: color-mix(in srgb, var(--c-accent) 55%, transparent);
}

/* 次级按钮统一到 30px 行高，与输入框对齐（全局 .btn 是 34px，混在一行里会高低不齐） */
.btn-sm {
  min-height: 30px;
  padding: 0 12px;
  font-size: var(--fs-meta);
}

.inp-num {
  width: 68px;
}

/* 数值单位：弱化到 --fg-2，避免和输入框里的数字抢 */
.unit {
  font-size: var(--fs-meta);
  color: var(--fg-2);
}

.sel {
  width: 100%;
  cursor: pointer;
}

.dest-path {
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

.ct-note {
  margin: 8px 0 0;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.5;
}

/* ---------- 动作 ---------- */
.ct-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 14px 0 4px;
}

/* 主操作：全页唯一一处实底主色按钮。
   颜色用 --color-primary / --color-on-primary 这对**全局配对**，不自己拿 --c-accent 拼：
   --c-accent 是"强调色"（浅色主题下会变成深赭 #7d571a），拿它当底色再配深色字
   会变成深字压深底（实测 3.07:1，不达标）；而 --color-primary 在两套主题下都是落日金，
   --color-on-primary 是配它的墨色 —— 和全站其它实底按钮一致。 */
.run-btn {
  height: 34px;
  padding: 0 18px;
  border: none;
  border-radius: var(--r-pill);
  background: var(--color-primary);
  color: var(--color-on-primary, #2a1a08);
  font-size: var(--fs-body);
  font-weight: var(--fw-strong);
  cursor: pointer;
  transition: filter var(--dur-2) var(--ease-out), opacity var(--dur-2) var(--ease-out);
}
.run-btn:hover:not(:disabled) {
  filter: brightness(1.08);
}
.run-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.ct-hint {
  font-size: var(--fs-meta);
  color: var(--fg-2);
}

/* ---------- 结果 ---------- */
.res {
  margin-top: 16px;
  border-radius: var(--r-ui);
  border: 1px solid var(--c-rule);
  overflow: hidden;
}

.res-sum {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 12px 14px;
  border-bottom: 1px solid var(--c-rule);
  font-size: var(--fs-body);
  color: var(--fg);
  flex-wrap: wrap;
}
.res-sum strong {
  font-weight: var(--fw-em);
}
.res-save {
  color: var(--c-accent);
  font-weight: var(--fw-em);
  font-variant-numeric: tabular-nums;
}
.res-err {
  color: var(--c-danger);
}

.res-list {
  margin: 0;
  padding: 0;
  list-style: none;
  max-height: 320px;
  overflow-y: auto;
}

.res-row {
  display: grid;
  grid-template-columns: minmax(120px, 1fr) 110px 150px 46px auto;
  align-items: center;
  gap: 10px;
  padding: 7px 14px;
  font-size: var(--fs-meta);
  font-variant-numeric: tabular-nums;
}
.res-row + .res-row {
  border-top: 1px solid var(--c-rule);
}

.res-name {
  color: var(--fg);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.res-dim,
.res-q {
  color: var(--fg-2);
}

.res-size {
  color: var(--fg);
}

.res-warn {
  color: var(--c-danger);
  font-size: 10px;
  padding: 1px 6px;
  border-radius: var(--r-pill);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-danger) 45%, transparent);
  justify-self: start;
}

.res-list-fail .res-row {
  grid-template-columns: minmax(120px, 1fr) auto;
}

/* ---------- 后台进度（运行中） ---------- */
.ct-progress {
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 14px 0 4px;
}

.ct-bar {
  position: relative;
  flex: 1;
  height: 6px;
  border-radius: var(--r-pill);
  background: var(--hover, rgba(120, 120, 130, 0.12));
  overflow: hidden;
}

.ct-bar > i {
  position: absolute;
  inset: 0 auto 0 0;
  display: block;
  border-radius: inherit;
  background: var(--color-primary);
  transition: width var(--dur-2) var(--ease-out);
}

.ct-progress-text {
  font-size: var(--fs-meta);
  color: var(--fg-2);
  font-variant-numeric: tabular-nums;
  max-width: 360px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 窄窗（≤1080）：参数从"四列并排"改成"标签左 / 控件右"的表单行。
   两列并排时，下拉框要么被拉成 400px 的空条，要么被限宽后右边空一块；
   表单行没有这个矛盾 —— 标签占固定宽，控件占剩下的，路径框自然铺满。 */
@media (max-width: 1080px) {
  .ct-opts {
    grid-template-columns: minmax(0, 1fr);
    gap: 8px 12px;
  }
  .field,
  .field-dest {
    grid-column: auto;
    display: grid;
    grid-template-columns: 72px minmax(0, 1fr);
    align-items: center;
    gap: 10px;
  }
  .sel {
    max-width: 220px;
  }
  .res-row {
    grid-template-columns: minmax(100px, 1fr) 90px 130px;
  }
  .res-q,
  .res-warn {
    display: none;
  }
}
</style>
