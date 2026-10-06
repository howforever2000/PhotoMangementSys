<script setup lang="ts">
/**
 * FEAT-095：照片删除方式选择弹窗（需求 R7 / D1）
 *
 * 删除必须二次确认，且提供两种模式让用户明确后果：
 *  - **相册记录删除**：清除扫描 / AI 记录与缩略图缓存，本地文件保留（可找回）
 *  - **本地删除（移入回收站）**：照片移入系统回收站，可找回；记录与缓存同步清除
 *
 * 原子性：三种实现此前散落在 PhotoGrid（record/file 两命令 + ConfirmDialog）、
 * PersonGallery、CategoryGallery（各自的 del-mask 弹窗），语义略有差异。
 * 本组件把「选模式」这一层收敛为唯一入口，各调用方只负责传路径 + 处理结果。
 */
import { useThemeStore } from "../stores/theme";

export type DeleteMode = "records" | "trash";

const props = defineProps<{
  /** 待删除路径；null = 关闭 */
  paths: string[] | null;
  /** 当前所在相册是虚拟相册（归档 / 隐藏）时为 true：
   *  此时没有「原相册」，只提供本地删除（记录删除对虚拟相册无意义） */
  inVirtualAlbum?: boolean;
  /** 正在执行删除（禁用按钮，防重复提交） */
  busy?: boolean;
}>();

const emit = defineEmits<{
  /** 用户选定删除方式，由调用方执行实际删除 */
  (e: "pick", mode: DeleteMode): void;
  (e: "cancel"): void;
}>();

const theme = useThemeStore();

const count = () => props.paths?.length ?? 0;
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
        <h4 class="pdd-title">选择删除方式（{{ count() }} 张）</h4>
        <p class="pdd-tip">两种方式都会从当前视图中移除照片，区别在于本地文件是否保留。</p>

        <button class="pdd-opt" :disabled="busy" @click="emit('pick', 'records')">
          <b>📄 从相册管理中删除（保留文件）</b>
          <span>清除扫描 / AI 记录与缩略图缓存，本地文件保留，可重新扫描找回</span>
        </button>

        <button class="pdd-opt pdd-opt-danger" :disabled="busy" @click="emit('pick', 'trash')">
          <b>🗑 本地删除（移入回收站）</b>
          <span>照片移入系统回收站，可在系统回收站找回；记录与缓存同步清除</span>
        </button>

        <button class="btn pdd-cancel" :disabled="busy" @click="emit('cancel')">
          {{ busy ? "处理中…" : "取消" }}
        </button>
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
  transition: border-color 0.15s, background 0.15s;
}
.pdd-opt:hover:not(:disabled) {
  border-color: rgba(106, 141, 240, 0.75);
  background: rgba(106, 141, 240, 0.08);
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
</style>
