<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import type { PersonInfo } from "../types/photo";
import { useNotify } from "../composables/useNotify";

defineProps<{ persons: PersonInfo[] }>();
const emit = defineEmits<{ refresh: [] }>();
const notify = useNotify();

const renamePerson = async (ps: PersonInfo, name: string) => {
  const n = name.trim();
  if (!n) return;
  try {
    await invoke("rename_person", { pid: ps.id, name: n });
    ps.name = n;
    emit("refresh");
    notify.success("重命名成功", `已重命名为 ${n}`);
  } catch (e) {
    notify.error("重命名失败", String(e));
  }
};

const askMerge = async (ps: PersonInfo) => {
  const target = prompt(`将 ${ps.id} 并入哪个标号？\n（输入目标标号，如 P001）`);
  if (!target?.trim()) return;
  try {
    await invoke("merge_persons", { target: target.trim(), source: ps.id });
    emit("refresh");
    notify.success("合并完成", `${ps.id} 已并入 ${target.trim()}`);
  } catch (e) {
    notify.error("合并失败", String(e));
  }
};

const removePerson = async (ps: PersonInfo) => {
  const ok = await notify.confirm(
    "删除人物",
    `确定删除人物 ${ps.id}（${ps.name}）？其标号将从已识别照片中移除`,
    { type: "danger", confirmText: "删除" },
  );
  if (!ok) return;
  try {
    await invoke("delete_person", { pid: ps.id });
    emit("refresh");
    notify.success("已删除人物", ps.id);
  } catch (e) {
    notify.error("删除失败", String(e));
  }
};
</script>

<template>
  <div v-if="persons.length" class="persons-panel">
    <div class="persons-head">
      <span class="persons-title">👤 人物注册表（同人同标号 · 可重命名/合并/删除）</span>
      <button class="persons-refresh" @click="emit('refresh')">刷新</button>
    </div>
    <div class="persons-grid">
      <div v-for="ps in persons" :key="ps.id" class="person-card">
        <div class="person-card-top">
          <span class="person-card-id mono">{{ ps.id }}</span>
          <span class="person-card-name">{{ ps.name || ps.id }}</span>
          <span class="person-card-count">{{ ps.face_count }} 张脸</span>
        </div>
        <div class="person-card-actions">
          <input
            class="person-rename-input"
            :placeholder="'重命名 ' + ps.id"
            @keyup.enter="renamePerson(ps, ($event.target as HTMLInputElement).value)"
          />
          <button class="btn-mini" @click="askMerge(ps)">并入他</button>
          <button class="btn-mini danger" @click="removePerson(ps)">删除</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.persons-panel {
  margin-top: 16px;
  border-top: 1px solid #e5e7eb;
  padding-top: 12px;
}

.persons-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}

.persons-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text);
}

.persons-refresh {
  background: transparent;
  border: 1px solid var(--color-border);
  border-radius: 4px;
  padding: 2px 8px;
  font-size: 11px;
  cursor: pointer;
  color: var(--color-text);
}

.persons-refresh:hover {
  background: var(--color-primary-soft);
}

.persons-grid {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.person-card {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 8px 10px;
  border: 1px solid var(--color-border);
  border-radius: 6px;
  background: var(--color-surface);
  min-width: 140px;
  max-width: 200px;
  color: var(--color-text);
}

.person-card-top {
  display: flex;
  align-items: center;
  gap: 6px;
}

.person-card-id {
  font-size: 11px;
  font-weight: 600;
  color: var(--color-text-2);
}

.person-card-name {
  font-size: 13px;
  font-weight: 500;
  color: var(--color-text);
}

.person-card-count {
  font-size: 11px;
  color: var(--color-text-2);
  margin-left: auto;
}

.person-card-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  align-items: center;
}

.person-rename-input {
  flex: 1;
  min-width: 60px;
  padding: 2px 4px;
  border: 1px solid var(--color-border);
  border-radius: 3px;
  font-size: 11px;
  outline: none;
  background: var(--color-surface);
  color: var(--color-text);
}

.person-rename-input:focus {
  border-color: #396cd8;
}

.btn-mini {
  padding: 2px 6px;
  font-size: 11px;
  border: 1px solid var(--color-border);
  border-radius: 3px;
  background: var(--color-surface);
  cursor: pointer;
  color: var(--color-text);
}

.btn-mini.danger {
  color: #e5484d;
  border-color: #e5484d;
}

.btn-mini:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>