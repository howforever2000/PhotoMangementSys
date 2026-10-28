<script setup lang="ts">
/**
 * 照片「处理方式」选择弹窗
 *
 * 把三件不同的事拆开，让用户清楚自己在删什么：
 *  - **移除（remove）**：只改「出现在哪里」，识别数据与文件都保留，可撤销
 *  - **修正（reassign）**：人工纠正 AI 认错的人，只改归属，不删任何数据
 *  - **清除（purge）**：删掉照片的全部识别数据，重新导入相册才回来
 *  - **删除文件（trash）**：移入系统回收站，识别数据同步清除
 *
 * 各页面按语境只显示适用的项：
 *  - 相册：移除 / 清除 / 回收站（3）
 *  - 人物：移除 / 移到其他人物 / 清除 / 回收站（4）
 *  - 分类：从分类移除 / 清除 / 回收站（3）
 *  - 地点、通用右键菜单：只有清除 / 回收站（2）—— 没有对应的「移除」数据模型，
 *    宁可少一个选项，也不能用一个语义不对的选项把文件真删掉（BUG-2026-1028-015）
 * 危险程度自上而下递增，最后一项永远是回收站。
 *
 * 原子性：以前四个画廊各自实现删除弹窗、语义还互有出入（详见
 * `delete_photo_records` 的 P0 修复）；现在「选方式」只剩这一处。
 */
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useThemeStore } from "../stores/theme";

export type DeleteMode = "remove" | "reassign" | "purge" | "trash";
/**
 * 页面语境：决定选项列表与文案
 *
 * - `album`：相册页，有真实的「原相册」概念 → 移除 = 写排除表（可撤销）
 * - `person`：人物页，多一项「移到其他人物」
 * - `category`：分类页 → 移除 = 删该分类的命中行（照片与其他识别数据都留）
 * - `location`：地点页，**没有**「只是不想在这里看到」的数据模型 → 不提供移除
 * - `generic`：跨视图的通用菜单（右键菜单），无法确定语境 → 不提供移除
 *   （BUG-2026-1028-015：这两类以前也能选「移除」，而执行层一律落到
 *    `delete_photos_to_trash`，等于「只想隐藏」却把文件删进了回收站）
 */
export type DeleteContext = "album" | "person" | "category" | "location" | "generic";

/** 可改挂的人物选项（只取展示所需字段，避免与 PersonInfo 强耦合） */
export interface PersonPickOption {
  id: string;
  name: string;
  face_count: number;
}

const props = defineProps<{
  /** 待处理路径；null = 关闭 */
  paths: string[] | null;
  /** 页面语境：决定选项列表与中间项文案 */
  context?: DeleteContext;
  /** 当前在虚拟相册（归档 / 隐藏）内：「移除」= 取消标记 */
  inVirtualAlbum?: boolean;
  /** 人物语境下当前人物名字（用于「从「张三」移除」文案） */
  personName?: string;
  /** 人物语境下的候选人物（不含当前人物） */
  persons?: PersonPickOption[];
  /** 正在执行（禁用按钮，防重复提交） */
  busy?: boolean;
}>();

const emit = defineEmits<{
  /** 选定处理方式，由调用方执行 */
  (e: "pick", mode: DeleteMode): void;
  /** 人物语境「移到其他人物」：pid = 目标人物；name = 新建人物名字（二选一） */
  (e: "reassign", target: { pid?: string; name?: string }): void;
  (e: "cancel"): void;
}>();

const theme = useThemeStore();

const count = computed(() => props.paths?.length ?? 0);
const context = computed<DeleteContext>(() => props.context ?? "album");

/** 子视图："modes" 选项列表 / "reassign" 人物选择器 */
const view = ref<"modes" | "reassign">("modes");
const keyword = ref("");
const newName = ref("");

// 每次打开都回到第一屏，清掉上次的搜索词
watch(
  () => props.paths,
  (p) => {
    if (p) {
      view.value = "modes";
      keyword.value = "";
      newName.value = "";
    }
  },
);

const personLabel = computed(() => props.personName?.trim() || "该人物");

/** 「只是不想在这里看到」只在有对应数据模型的语境里成立 */
const supportsRemove = computed(
  () => context.value === "album" || context.value === "person" || context.value === "category",
);

const removeTitle = computed(() => {
  if (context.value === "person") return `从「${personLabel.value}」移除`;
  if (context.value === "category") return "从这个分类移除";
  return "从这个相册移除";
});

const removeHint = computed(() => {
  if (context.value === "person")
    return "只解除这张照片与该人物的关联（修正会被记住，重扫不会再挂回此人），照片与其他识别数据都不动";
  if (context.value === "category")
    return "只删掉「这张照片属于这个分类」这一条判断，照片、其他分类、人物与向量全部保留";
  return props.inVirtualAlbum
    ? "取消归档 / 隐藏，照片回到原相册；识别数据与文件都不动"
    : "照片不再出现在本相册，识别数据与文件都保留，可随时恢复";
});

const filteredPersons = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  const list = props.persons ?? [];
  return kw ? list.filter((p) => p.name.toLowerCase().includes(kw) || p.id.toLowerCase().includes(kw)) : list;
});

function pick(mode: DeleteMode) {
  if (props.busy) return;
  if (mode === "reassign") {
    view.value = "reassign";
    return;
  }
  emit("pick", mode);
}

function choosePerson(p: PersonPickOption) {
  if (props.busy) return;
  emit("reassign", { pid: p.id });
}

function chooseNew() {
  const name = newName.value.trim();
  if (!name || props.busy) return;
  emit("reassign", { name });
}

/** Esc 关闭（其余弹窗同一口径：只在打开时挂 document 监听） */
function onKey(e: KeyboardEvent) {
  if (e.key !== "Escape" || !props.paths) return;
  e.preventDefault();
  e.stopPropagation();
  if (view.value === "reassign") {
    view.value = "modes";
    return;
  }
  if (!props.busy) emit("cancel");
}
onMounted(() => document.addEventListener("keydown", onKey));
onUnmounted(() => document.removeEventListener("keydown", onKey));
</script>

<template>
  <Teleport to="body">
    <div v-if="paths" class="pdd-mask" @click.self="emit('cancel')">
      <div
        class="pdd-dialog"
        role="dialog"
        aria-modal="true"
        :style="{ background: theme.isDark ? 'rgba(30,34,46,.96)' : '#fff' }"
      >
        <!-- 第一屏：选处理方式 -->
        <template v-if="view === 'modes'">
          <h4 class="pdd-title">处理这张照片（{{ count }} 张）</h4>
          <p class="pdd-tip">先想清楚是「只是不想在这里看到」，还是「要把识别数据删掉」。</p>

          <button v-if="supportsRemove" class="pdd-opt" :disabled="busy" @click="pick('remove')">
            <b>👁 {{ removeTitle }}</b>
            <span>{{ removeHint }}</span>
          </button>

          <button
            v-if="context === 'person'"
            class="pdd-opt"
            :disabled="busy"
            @click="pick('reassign')"
          >
            <b>🔁 移到其他人物…</b>
            <span>AI 认错了人？把它挂到正确的人名下。修正结果在重新扫描后依然有效</span>
          </button>

          <button class="pdd-opt pdd-opt-warn" :disabled="busy" @click="pick('purge')">
            <b>🧹 清除数据库记录</b>
            <span>删掉全部识别数据（标签 / 分类 / 人物 / 语义向量）并让它从相册消失；重新导入相册才能找回</span>
          </button>

          <button class="pdd-opt pdd-opt-danger" :disabled="busy" @click="pick('trash')">
            <b>🗑 删除文件（移入回收站）</b>
            <span>照片移入系统回收站，可在回收站找回；识别数据同步清除</span>
          </button>

          <button class="btn pdd-cancel" :disabled="busy" @click="emit('cancel')">
            {{ busy ? "处理中…" : "取消" }}
          </button>
        </template>

        <!-- 第二屏：选目标人物 -->
        <template v-else>
          <h4 class="pdd-title">移到哪个「人物」？（{{ count }} 张）</h4>
          <p class="pdd-tip">选一个已有的人物，或新建一个人物并命名。</p>

          <input v-model="keyword" class="pdd-input" type="search" placeholder="搜索人物…" />
          <div class="pdd-person-list">
            <button
              v-for="p in filteredPersons"
              :key="p.id"
              class="pdd-person"
              :disabled="busy"
              @click="choosePerson(p)"
            >
              <span class="pdd-person-name">{{ p.name }}</span>
              <span class="pdd-person-count">{{ p.face_count }} 张</span>
            </button>
            <p v-if="!filteredPersons.length" class="pdd-empty">没有匹配的人物</p>
          </div>

          <div class="pdd-new">
            <input v-model="newName" class="pdd-input" type="text" placeholder="或新建人物（输入名字）" />
            <button class="btn btn-primary" :disabled="busy || !newName.trim()" @click="chooseNew">
              新建并移入
            </button>
          </div>

          <button class="btn pdd-cancel" :disabled="busy" @click="view = 'modes'">返回</button>
        </template>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.pdd-mask {
  position: fixed;
  inset: 0;
  z-index: 1250;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
}
.pdd-dialog {
  width: min(440px, 92vw);
  max-height: 86vh;
  overflow-y: auto;
  border-radius: 14px;
  padding: 18px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.28);
}
.pdd-title {
  margin: 0;
  font-size: 16px;
  font-weight: 700;
}
.pdd-tip {
  margin: -4px 0 4px;
  font-size: 12px;
  opacity: 0.65;
  line-height: 1.5;
}
.pdd-opt {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  padding: 12px 14px;
  border-radius: 10px;
  border: 1px solid rgba(127, 127, 127, 0.32);
  background: transparent;
  color: inherit;
  cursor: pointer;
  text-align: left;
  transition: border-color var(--dur-2), background var(--dur-2);
}
.pdd-opt:hover:not(:disabled) {
  border-color: rgba(106, 141, 240, 0.75);
  background: rgba(106, 141, 240, 0.08);
}
.pdd-opt-warn:hover:not(:disabled) {
  border-color: color-mix(in srgb, #e0a03a 75%, transparent);
  background: color-mix(in srgb, #e0a03a 10%, transparent);
}
.pdd-opt-danger:hover:not(:disabled) {
  border-color: color-mix(in srgb, var(--color-danger) 75%, transparent);
  background: color-mix(in srgb, var(--color-danger) 8%, transparent);
}
.pdd-opt span {
  font-size: 12px;
  opacity: 0.65;
  line-height: 1.5;
}
.pdd-opt:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.pdd-cancel {
  align-self: flex-end;
}
.pdd-input {
  width: 100%;
  box-sizing: border-box;
  padding: 8px 10px;
  border-radius: 8px;
  border: 1px solid rgba(127, 127, 127, 0.32);
  background: transparent;
  color: inherit;
}
.pdd-person-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 40vh;
  overflow-y: auto;
}
.pdd-person {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 9px 12px;
  border-radius: 8px;
  border: 1px solid rgba(127, 127, 127, 0.24);
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.pdd-person:hover:not(:disabled) {
  border-color: rgba(106, 141, 240, 0.75);
  background: rgba(106, 141, 240, 0.08);
}
.pdd-person-count {
  font-size: 12px;
  opacity: 0.6;
}
.pdd-empty {
  margin: 4px 0;
  font-size: 12px;
  opacity: 0.6;
}
.pdd-new {
  display: flex;
  gap: 8px;
}
.pdd-new .pdd-input {
  flex: 1;
}
</style>
