<script setup lang="ts">
/**
 * 人物画廊 —— 智慧相册「人物」分类的展示组件
 *
 * 数据源：persons.db 直读（list_persons，按脸数降序）；头像本地裁剪缓存。
 * 支持：行内重命名 / 合并到其他人物（二次确认）。
 * 与 ScanPanel 内 PersonPanel 的差异：本组件面向浏览场景，完全离线可用。
 */
import { computed, onMounted, ref, type Directive } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { PersonInfo } from "../types/photo";
import ConfirmDialog from "./ConfirmDialog.vue";
import { useThemeStore } from "../stores/theme";

const theme = useThemeStore();
/** 卡片/弹窗底色：跟随主题浅深模式，保证文字始终可读 */
const surfaceStyle = computed(() => theme.cardStyle);
const persons = ref<PersonInfo[]>([]);
const loading = ref(true);
const loadError = ref("");
const actionMsg = ref("");
/** pid → 头像 URL（获取失败无键，回退首字占位） */
const avatarMap = ref<Record<string, string>>({});

/* ---- 行内重命名 ---- */
const editingId = ref<string | null>(null);
const editingName = ref("");

function startRename(p: PersonInfo) {
  editingId.value = p.id;
  editingName.value = p.name && p.name !== p.id ? p.name : "";
}

async function saveRename(p: PersonInfo) {
  const name = editingName.value.trim();
  if (!name) return stopRename();
  try {
    await invoke("rename_person", { pid: p.id, name });
    p.name = name;
    flash(`已重命名为「${name}」`);
  } catch (e) {
    flash(`重命名失败：${String(e)}`);
  } finally {
    stopRename();
  }
}

function stopRename() {
  editingId.value = null;
  editingName.value = "";
}

/* ---- 合并：选中的源人物 → 弹窗挑目标 → 二次确认 ---- */
const mergingSource = ref<PersonInfo | null>(null);
/** 已选定待确认的目标 */
const pendingTarget = ref<PersonInfo | null>(null);
const mergingBusy = ref(false);

const mergeCandidates = () => persons.value.filter((p) => p.id !== mergingSource.value?.id);

function displayName(p: PersonInfo): string {
  return p.name && p.name !== p.id ? p.name : p.id;
}

async function doMerge() {
  if (!mergingSource.value || !pendingTarget.value || mergingBusy.value) return;
  const source = mergingSource.value;
  const target = pendingTarget.value;
  mergingBusy.value = true;
  try {
    await invoke("merge_persons", { target: target.id, source: source.id });
    // 目标头像的人脸集合已变化 → 强制刷新缓存
    const fresh = await invoke<string>("get_person_avatar", {
      pid: target.id,
      forceRefresh: true,
    });
    avatarMap.value = { ...avatarMap.value, [target.id]: convertFileSrc(fresh) };
    flash(`已将 ${displayName(source)}（${source.face_count} 张脸）并入 ${displayName(target)}`);
    await load();
  } catch (e) {
    flash(`合并失败：${String(e)}`);
  } finally {
    mergingBusy.value = false;
    pendingTarget.value = null;
    mergingSource.value = null;
  }
}

/* ---- 加载 ---- */
async function load() {
  loading.value = true;
  loadError.value = "";
  try {
    persons.value = await invoke<PersonInfo[]>("list_persons");
    for (const p of persons.value) {
      if (avatarMap.value[p.id]) continue;
      try {
        const cachePath = await invoke<string>("get_person_avatar", {
          pid: p.id,
          forceRefresh: false,
        });
        avatarMap.value = { ...avatarMap.value, [p.id]: convertFileSrc(cachePath) };
      } catch {
        /* 原图缺失等：回退占位 */
      }
    }
  } catch (e) {
    loadError.value = String(e);
  } finally {
    loading.value = false;
  }
}

function flash(msg: string) {
  actionMsg.value = msg;
  setTimeout(() => (actionMsg.value = ""), 4000);
}

onMounted(load);

/** 本地指令：进入重命名时自动聚焦 */
const vFocus: Directive<HTMLElement> = {
  mounted: (el) => el.focus(),
};
</script>

<template>
  <div class="pg-wrap" :style="{ color: theme.textColor }">
    <div class="pg-toolbar">
      <span class="pg-summary" v-if="!loading && !loadError">共 {{ persons.length }} 位人物（按出现次数排序）</span>
      <button class="btn pg-refresh-btn" @click="load">刷新</button>
    </div>
    <div v-if="actionMsg" class="pg-action-msg">{{ actionMsg }}</div>

    <!-- 加载中 -->
    <div v-if="loading" class="pg-state">正在读取人物注册表…</div>

    <!-- 加载失败 -->
    <div v-else-if="loadError" class="pg-state pg-error">
      读取失败：{{ loadError }}
      <p class="pg-hint">请确认项目目录下存在 python/data/persons.db（执行过人脸识别扫描）。</p>
    </div>

    <!-- 空状态 -->
    <div v-else-if="!persons.length" class="pg-state">
      <p>暂无已识别的人物。</p>
      <p class="pg-hint">进入任意相册 → 「综合扫描」（勾选人脸识别）后，识别到的人物会自动登记在此。</p>
    </div>

    <!-- 人物卡片网格 -->
    <div v-else class="person-grid">
      <article v-for="p in persons" :key="p.id" class="person-card" :style="surfaceStyle" :title="`${displayName(p)}（${p.id}）`">
        <div class="person-avatar-wrap">
          <img v-if="avatarMap[p.id]" :src="avatarMap[p.id]" class="person-avatar" alt="" />
          <span v-else class="person-avatar person-avatar-fallback">{{ displayName(p).slice(0, 1) }}</span>
          <span class="person-face-count">{{ p.face_count }} 张脸</span>
        </div>
        <div class="person-info">
          <!-- 行内重命名 -->
          <template v-if="editingId === p.id">
            <input
              v-model="editingName"
              class="person-rename-input"
              maxlength="50"
              placeholder="输入新名称"
              @keyup.enter="saveRename(p)"
              @keyup.esc="stopRename"
              v-focus
            />
            <div class="person-edit-actions">
              <button class="mini-btn ok" @click="saveRename(p)">保存</button>
              <button class="mini-btn" @click="stopRename">取消</button>
            </div>
          </template>
          <template v-else>
            <div class="person-name-row">
              <span class="person-name">{{ displayName(p) }}</span>
              <button class="mini-btn" title="重命名" @click="startRename(p)">✎</button>
            </div>
            <div class="person-id mono">{{ p.id }}</div>
            <div class="person-date">登记于 {{ p.created_at }}</div>
            <button class="mini-btn merge-btn" @click="mergingSource = p">合并到其他人物…</button>
          </template>
        </div>
      </article>
    </div>

    <!-- 合并：选择目标人物弹窗 -->
    <Teleport to="body">
      <div v-if="mergingSource && !pendingTarget" class="merge-mask" @click.self="mergingSource = null">
        <div class="merge-dialog" :style="surfaceStyle">
          <h4>将 {{ displayName(mergingSource) }}（{{ mergingSource.face_count }} 张脸）合并到…</h4>
          <p class="merge-tip">选择保留的目标人物；被并者的人脸与计数将全部转移。</p>
          <div class="merge-list">
            <button
              v-for="c in mergeCandidates()"
              :key="c.id"
              class="merge-item"
              @click="pendingTarget = c"
            >
              <img v-if="avatarMap[c.id]" :src="avatarMap[c.id]" class="merge-avatar" alt="" />
              <span v-else class="merge-avatar merge-avatar-fallback">{{ displayName(c).slice(0, 1) }}</span>
              <span class="merge-name">{{ displayName(c) }}</span>
              <span class="mono merge-id">{{ c.id }}</span>
              <span class="merge-count">{{ c.face_count }} 张脸</span>
            </button>
          </div>
          <div class="merge-actions">
            <button class="btn" @click="mergingSource = null">取消</button>
          </div>
        </div>
      </div>
    </Teleport>

    <!-- 合并二次确认 -->
    <ConfirmDialog
      :visible="!!pendingTarget && !!mergingSource"
      title="合并人物"
      :message="
        pendingTarget && mergingSource
          ? `确定将 ${displayName(mergingSource)}（${mergingSource.face_count} 张脸）并入 ${displayName(pendingTarget)}（${pendingTarget.face_count} 张脸）吗？合并后不可自动拆分。`
          : ''
      "
      confirm-text="确认合并"
      :danger="false"
      @confirm="doMerge"
      @cancel="pendingTarget = null"
    />
  </div>
</template>

<style scoped>
.pg-wrap { min-width: 0; }

.pg-toolbar {
  display: flex;
  align-items: center;
  margin-bottom: 12px;
}
.pg-summary {
  opacity: 0.75;
  font-size: 13px;
}
.pg-refresh-btn { margin-left: auto; padding: 4px 12px; font-size: 13px; }

.pg-action-msg {
  background: #eef5ff;
  border: 1px solid #d3e3ff;
  color: #2f5bc0;
  border-radius: 8px;
  padding: 8px 12px;
  font-size: 13px;
  margin-bottom: 12px;
}

.pg-state {
  text-align: center;
  padding: 48px 20px;
  opacity: 0.8;
}
.pg-error { color: #e5484d; }
.pg-hint {
  opacity: 0.65;
  font-size: 13px;
  margin-top: 8px;
}

.person-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
  gap: 16px;
}

.person-card {
  background: transparent; /* 底色由 surfaceStyle(主题) 提供 */
  border-radius: 12px;
  padding: 16px;
  display: flex;
  align-items: center;
  gap: 14px;
  transition: transform 0.15s, box-shadow 0.15s;
}
.person-card:hover {
  transform: translateY(-2px);
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.1);
}

.person-avatar-wrap { position: relative; flex-shrink: 0; }

.person-avatar {
  width: 64px;
  height: 64px;
  border-radius: 50%;
  object-fit: cover;
  display: block;
  border: 2px solid rgba(57, 108, 216, 0.35);
}
.person-avatar-fallback {
  background: #396cd8;
  color: #fff;
  font-size: 26px;
  line-height: 60px;
  text-align: center;
}
.person-face-count {
  position: absolute;
  bottom: -4px;
  left: 50%;
  transform: translateX(-50%);
  background: #396cd8;
  color: #fff;
  font-size: 11px;
  padding: 1px 8px;
  border-radius: 999px;
  white-space: nowrap;
}

.person-info { min-width: 0; flex: 1; }

.person-name-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.person-name {
  font-size: 15px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mono { font-family: Consolas, Monaco, monospace; }
.person-id { font-size: 12px; color: #396cd8; margin-top: 2px; }
.person-date { font-size: 11px; opacity: 0.6; margin-top: 4px; }

.mini-btn {
  border: 1px solid currentColor;
  background: transparent;
  border-radius: 6px;
  font-size: 11px;
  padding: 2px 7px;
  cursor: pointer;
  color: inherit;
  transition: all 0.15s;
  flex-shrink: 0;
}
.mini-btn:hover { border-color: #396cd8; color: #396cd8; }
.mini-btn.ok { background: #396cd8; border-color: #396cd8; color: #fff; }
.merge-btn { margin-top: 6px; }

.person-rename-input {
  width: 100%;
  font-size: 13px;
  padding: 3px 8px;
  border: 1px solid #396cd8;
  border-radius: 6px;
  outline: none;
}
.person-edit-actions { display: flex; gap: 6px; margin-top: 6px; }

/* 合并选择弹窗 */
.merge-mask {
  position: fixed;
  inset: 0;
  z-index: 1100;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
}
.merge-dialog {
  background: transparent; /* 底色由 surfaceStyle(主题) 提供 */
  border-radius: 14px;
  padding: 20px 22px;
  width: min(420px, 92vw);
  max-height: 76vh;
  display: flex;
  flex-direction: column;
}
.merge-dialog h4 { margin: 0 0 6px; }
.merge-tip { font-size: 12px; opacity: 0.7; margin: 0 0 12px; }
.merge-list { overflow-y: auto; display: flex; flex-direction: column; gap: 8px; }
.merge-item {
  display: flex;
  align-items: center;
  gap: 10px;
  box-shadow: inset 0 0 0 1px rgba(128,138,158,.4);
  background: transparent;
  border-radius: 10px;
  padding: 8px 12px;
  cursor: pointer;
  text-align: left;
  transition: all 0.15s;
}
.merge-item:hover { border-color: #396cd8; background: rgba(57, 108, 216, 0.06); }
.merge-avatar {
  width: 36px;
  height: 36px;
  border-radius: 50%;
  object-fit: cover;
}
.merge-avatar-fallback {
  background: #396cd8;
  color: #fff;
  line-height: 34px;
  text-align: center;
  font-size: 15px;
}
.merge-name { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.merge-id { font-size: 11px; color: #396cd8; }
.merge-count { margin-left: auto; font-size: 12px; opacity: 0.7; }
.merge-actions { margin-top: 14px; display: flex; justify-content: flex-end; }
</style>
