<script setup lang="ts">
/**
 * 开发者视角 · 数据与路径（副窗口，label=dev-data）
 *
 * 目的：把「数据到底写在哪、里面有什么」变成可直读的界面，替代翻代码/猜路径：
 *   - 左栏：运行态路径清单（DB / 缓存 / 模型 / 运行态），含体量、文件数、口径说明，
 *     可复制、可在资源管理器中定位
 *   - 右栏：两个库（photos.db / persons.db）的表清单与数据预览
 *
 * 安全边界（后端强制，前端只做呈现）：
 *   - 全部只读（SQLITE_OPEN_READONLY）；不可写、无任意 SQL 入口
 *   - 表名走 sqlite_master 校验；行数上限 200；单元格截断；BLOB 只报字节数
 *   - 敏感列（password_hash / token 等）已由后端打码
 */
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

/** 一条运行态路径（对应 Rust devdata::PathEntry） */
interface PathEntry {
  key: string;
  group: string;
  label: string;
  path: string;
  exists: boolean;
  is_dir: boolean;
  bytes: number;
  files: number;
  note: string;
}

/** 表信息（对应 Rust devdata::TableInfo） */
interface TableInfo {
  name: string;
  rows: number;
  shadow: boolean;
}

/** 库信息（对应 Rust devdata::DbInfo） */
interface DbInfo {
  key: string;
  label: string;
  path: string;
  exists: boolean;
  bytes: number;
  tables: TableInfo[];
  error: string;
}

/** 预览结果（对应 Rust devdata::RowsResult） */
interface RowsResult {
  db: string;
  table: string;
  columns: string[];
  masked_columns: string[];
  rows: string[][];
  total: number;
  truncated: boolean;
}

const paths = ref<PathEntry[]>([]);
const dbs = ref<DbInfo[]>([]);
const loading = ref(false);
const errorMsg = ref("");
const notice = ref("");

const activeDb = ref("photos");
const activeTable = ref("");
const limit = ref(50);
const preview = ref<RowsResult | null>(null);
const previewLoading = ref(false);
const previewError = ref("");

const GROUP_ORDER = ["数据库", "缓存", "模型", "运行态"];

const grouped = computed(() => {
  const map = new Map<string, PathEntry[]>();
  for (const p of paths.value) {
    if (!map.has(p.group)) map.set(p.group, []);
    map.get(p.group)!.push(p);
  }
  return GROUP_ORDER.filter((g) => map.has(g)).map((g) => ({ group: g, items: map.get(g)! }));
});

const currentDb = computed(() => dbs.value.find((d) => d.key === activeDb.value) || null);

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

function fmtNum(n: number): string {
  if (n < 0) return "—";
  return n.toLocaleString("en-US");
}

async function copyText(text: string, what: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // WebView2 下 clipboard 偶发不可用（非安全上下文/焦点问题）：退回到临时 textarea
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.cssText = "position:fixed;opacity:0";
    document.body.appendChild(ta);
    ta.select();
    try {
      document.execCommand("copy");
    } finally {
      document.body.removeChild(ta);
    }
  }
  notice.value = `已复制${what}`;
  window.setTimeout(() => (notice.value = ""), 1500);
}

async function reveal(key: string) {
  try {
    await invoke("dev_reveal_path", { key });
  } catch (e) {
    errorMsg.value = `定位失败: ${e}`;
  }
}

async function loadPaths() {
  try {
    paths.value = await invoke<PathEntry[]>("dev_data_paths");
  } catch (e) {
    errorMsg.value = `读取路径清单失败: ${e}`;
  }
}

async function loadDbs() {
  try {
    dbs.value = await invoke<DbInfo[]>("dev_db_tables");
    if (!dbs.value.some((d) => d.key === activeDb.value)) {
      activeDb.value = dbs.value[0]?.key ?? "photos";
    }
    // 默认选中第一个非影子、且有数据的表，打开即见内容
    if (!activeTable.value) {
      const t = currentDb.value?.tables.find((x) => !x.shadow && x.rows > 0)
        ?? currentDb.value?.tables.find((x) => !x.shadow);
      if (t) void selectTable(t.name);
    }
  } catch (e) {
    errorMsg.value = `读取表清单失败: ${e}`;
  }
}

async function selectTable(name: string) {
  activeTable.value = name;
  previewLoading.value = true;
  previewError.value = "";
  try {
    preview.value = await invoke<RowsResult>("dev_db_rows", {
      db: activeDb.value,
      table: name,
      limit: limit.value,
    });
  } catch (e) {
    preview.value = null;
    previewError.value = String(e);
  } finally {
    previewLoading.value = false;
  }
}

async function switchDb(key: string) {
  activeDb.value = key;
  activeTable.value = "";
  preview.value = null;
  const t = currentDb.value?.tables.find((x) => !x.shadow && x.rows > 0)
    ?? currentDb.value?.tables.find((x) => !x.shadow);
  if (t) await selectTable(t.name);
}

async function refreshAll() {
  loading.value = true;
  errorMsg.value = "";
  await Promise.all([loadPaths(), loadDbs()]);
  if (activeTable.value) await selectTable(activeTable.value);
  loading.value = false;
}

onMounted(() => {
  void refreshAll();
});
</script>

<template>
  <div class="dw">
    <header class="dw-head">
      <div class="dw-title">
        <span class="dw-dot"></span>
        开发者视角 · 数据与路径
        <span class="dw-ro">只读</span>
      </div>
      <div class="dw-actions">
        <span v-if="notice" class="dw-notice">{{ notice }}</span>
        <button class="dw-btn" type="button" :disabled="loading" @click="refreshAll">
          {{ loading ? "刷新中…" : "刷新" }}
        </button>
      </div>
    </header>

    <p v-if="errorMsg" class="dw-err">{{ errorMsg }}</p>

    <div class="dw-body">
      <!-- 左：路径清单 -->
      <section class="dw-paths">
        <div v-for="g in grouped" :key="g.group" class="dw-group">
          <h3 class="dw-group-title">{{ g.group }}</h3>
          <article
            v-for="p in g.items"
            :key="p.key"
            class="dw-card"
            :class="{ 'dw-missing': !p.exists }"
          >
            <div class="dw-card-head">
              <b>{{ p.label }}</b>
              <span class="dw-badge" :class="p.exists ? 'ok' : 'no'">
                {{ p.exists ? (p.is_dir ? `${fmtNum(p.files)} 文件` : "存在") : "缺失" }}
              </span>
              <span class="dw-size">{{ fmtBytes(p.bytes) }}</span>
            </div>
            <div class="dw-path" :title="p.path" @click="copyText(p.path, '路径')">
              {{ p.path }}
            </div>
            <div class="dw-card-foot">
              <button class="dw-mini" type="button" @click="copyText(p.path, '路径')">复制</button>
              <button class="dw-mini" type="button" @click="reveal(p.key)">打开所在位置</button>
            </div>
            <p class="dw-note">{{ p.note }}</p>
          </article>
        </div>
      </section>

      <!-- 右：库浏览 -->
      <section class="dw-db">
        <div class="dw-tabs">
          <button
            v-for="d in dbs"
            :key="d.key"
            class="dw-tab"
            :class="{ active: d.key === activeDb }"
            type="button"
            @click="switchDb(d.key)"
          >
            {{ d.label }}
            <span class="dw-tab-meta" :class="{ bad: !d.exists }">
              {{ d.exists ? `${d.tables.length} 表 · ${fmtBytes(d.bytes)}` : "不存在" }}
            </span>
          </button>
        </div>

        <p v-if="currentDb" class="dw-dbpath" :title="currentDb.path" @click="copyText(currentDb.path, '库路径')">
          {{ currentDb.path }}
        </p>
        <p v-if="currentDb?.error" class="dw-err">{{ currentDb.error }}</p>

        <div class="dw-db-body">
          <div class="dw-tables">
            <button
              v-for="t in currentDb?.tables || []"
              :key="t.name"
              class="dw-table-item"
              :class="{ active: t.name === activeTable, shadow: t.shadow }"
              type="button"
              :title="t.shadow ? 'SQLite FTS 影子表（全文索引实现细节）' : t.name"
              @click="selectTable(t.name)"
            >
              <span class="dw-table-name">{{ t.name }}</span>
              <span class="dw-table-rows">{{ fmtNum(t.rows) }}</span>
            </button>
          </div>

          <div class="dw-preview">
            <div class="dw-preview-head">
              <span class="dw-preview-title">
                {{ preview ? `${preview.db} · ${preview.table}` : "选择左侧表名查看前 N 行" }}
              </span>
              <span v-if="preview" class="dw-preview-meta">
                共 {{ fmtNum(preview.total) }} 行
                <template v-if="preview.truncated">· 已截断</template>
                <template v-if="preview.masked_columns.length">
                  · 打码列：{{ preview.masked_columns.join(", ") }}
                </template>
              </span>
              <span class="dw-limit">
                显示
                <select v-model.number="limit" @change="activeTable && selectTable(activeTable)">
                  <option :value="20">20</option>
                  <option :value="50">50</option>
                  <option :value="200">200</option>
                </select>
                行
              </span>
            </div>

            <p v-if="previewError" class="dw-err">{{ previewError }}</p>
            <p v-else-if="previewLoading" class="dw-dim">读取中…</p>
            <p v-else-if="!preview" class="dw-dim">未选择表</p>
            <p v-else-if="preview.rows.length === 0" class="dw-dim">该表暂无数据</p>

            <div v-else class="dw-table-wrap">
              <table class="dw-grid">
                <thead>
                  <tr>
                    <th class="dw-idx">#</th>
                    <th
                      v-for="c in preview.columns"
                      :key="c"
                      :class="{ 'dw-masked-col': preview.masked_columns.includes(c) }"
                    >
                      {{ c }}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="(row, i) in preview.rows" :key="i">
                    <td class="dw-idx">{{ i + 1 }}</td>
                    <td
                      v-for="(cell, j) in row"
                      :key="j"
                      :class="{ 'dw-masked': preview.masked_columns.includes(preview.columns[j]) }"
                      :title="cell"
                    >
                      {{ cell }}
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </section>
    </div>
  </div>
</template>

<style scoped>
.dw {
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
.dw-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.dw-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  color: #e6ebf5;
}
.dw-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #4bd07a;
  box-shadow: 0 0 8px #4bd07a;
}
.dw-ro {
  padding: 1px 6px;
  border: 1px solid #2c3752;
  border-radius: 999px;
  font-size: 10px;
  color: #8aa0c0;
}
.dw-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}
.dw-notice {
  color: #4bd07a;
  font-size: 11px;
}
.dw-btn,
.dw-mini {
  background: #161b26;
  color: #cbd3e1;
  border: 1px solid #263044;
  border-radius: 4px;
  padding: 3px 10px;
  font: inherit;
  cursor: pointer;
}
.dw-btn:hover,
.dw-mini:hover {
  border-color: #3d5680;
  color: #fff;
}
.dw-mini {
  padding: 1px 8px;
  font-size: 11px;
}
.dw-err {
  margin: 0;
  padding: 6px 8px;
  border-left: 2px solid #ff6b6b;
  background: #1d1216;
  color: #ff9a9a;
  font-size: 11px;
  white-space: pre-wrap;
}
.dw-dim {
  color: #6b7688;
  margin: 6px 0;
}
.dw-body {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template-columns: minmax(300px, 34%) 1fr;
  gap: 10px;
}
.dw-paths {
  overflow: auto;
  padding-right: 4px;
}
.dw-group-title {
  margin: 10px 0 6px;
  font-size: 11px;
  font-weight: 600;
  color: #7f8ea8;
  letter-spacing: 0.08em;
}
.dw-card {
  border: 1px solid #1e2634;
  border-radius: 6px;
  padding: 7px 8px;
  margin-bottom: 7px;
  background: #111621;
}
.dw-card.dw-missing {
  opacity: 0.72;
  border-style: dashed;
}
.dw-card-head {
  display: flex;
  align-items: center;
  gap: 6px;
  color: #e6ebf5;
}
.dw-badge {
  font-size: 10px;
  padding: 0 5px;
  border-radius: 999px;
  border: 1px solid #2c3752;
}
.dw-badge.ok {
  color: #4bd07a;
  border-color: #245c39;
}
.dw-badge.no {
  color: #ff9a9a;
  border-color: #5c2424;
}
.dw-size {
  margin-left: auto;
  color: #8aa0c0;
  font-size: 11px;
}
.dw-path {
  margin: 5px 0;
  color: #9fb4d4;
  word-break: break-all;
  cursor: pointer;
  font-size: 11px;
}
.dw-path:hover {
  color: #d6e3ff;
}
.dw-card-foot {
  display: flex;
  gap: 6px;
}
.dw-note {
  margin: 6px 0 0;
  color: #6b7688;
  font-size: 10.5px;
  line-height: 1.45;
}
.dw-db {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.dw-tabs {
  display: flex;
  gap: 6px;
}
.dw-tab {
  flex: 1;
  text-align: left;
  background: #111621;
  border: 1px solid #1e2634;
  border-radius: 6px;
  padding: 6px 8px;
  color: #cbd3e1;
  font: inherit;
  cursor: pointer;
}
.dw-tab.active {
  border-color: #3d5680;
  background: #16203a;
  color: #fff;
}
.dw-tab-meta {
  display: block;
  color: #7f8ea8;
  font-size: 10.5px;
}
.dw-tab-meta.bad {
  color: #ff9a9a;
}
.dw-dbpath {
  margin: 0;
  color: #7f8ea8;
  font-size: 10.5px;
  word-break: break-all;
  cursor: pointer;
}
.dw-db-body {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template-columns: minmax(180px, 26%) 1fr;
  gap: 8px;
}
.dw-tables {
  overflow: auto;
  border: 1px solid #1e2634;
  border-radius: 6px;
  background: #0f131c;
  padding: 4px;
}
.dw-table-item {
  width: 100%;
  display: flex;
  justify-content: space-between;
  gap: 6px;
  background: transparent;
  border: 0;
  border-radius: 4px;
  padding: 3px 6px;
  color: #b9c4d6;
  font: inherit;
  cursor: pointer;
  text-align: left;
}
.dw-table-item:hover {
  background: #16203a;
}
.dw-table-item.active {
  background: #1c2947;
  color: #fff;
}
.dw-table-item.shadow {
  color: #6b7688;
}
.dw-table-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.dw-table-rows {
  color: #7f8ea8;
  font-size: 10.5px;
}
.dw-preview {
  min-width: 0;
  display: flex;
  flex-direction: column;
  border: 1px solid #1e2634;
  border-radius: 6px;
  background: #0f131c;
  padding: 6px 8px;
  overflow: hidden;
}
.dw-preview-head {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  padding-bottom: 5px;
  border-bottom: 1px solid #1a2130;
}
.dw-preview-title {
  color: #e6ebf5;
}
.dw-preview-meta {
  color: #7f8ea8;
  font-size: 10.5px;
}
.dw-limit {
  margin-left: auto;
  color: #7f8ea8;
  font-size: 10.5px;
}
.dw-limit select {
  background: #161b26;
  color: #cbd3e1;
  border: 1px solid #263044;
  border-radius: 3px;
  font: inherit;
  padding: 1px 2px;
}
.dw-table-wrap {
  flex: 1;
  min-height: 0;
  overflow: auto;
  margin-top: 5px;
}
.dw-grid {
  border-collapse: collapse;
  font-size: 11px;
  white-space: nowrap;
}
.dw-grid th,
.dw-grid td {
  border: 1px solid #1a2130;
  padding: 2px 7px;
  text-align: left;
  max-width: 340px;
  overflow: hidden;
  text-overflow: ellipsis;
}
.dw-grid th {
  position: sticky;
  top: 0;
  background: #16203a;
  color: #cfdcf3;
  z-index: 1;
}
.dw-grid tbody tr:nth-child(even) {
  background: #111621;
}
.dw-idx {
  color: #5d6a7e;
  text-align: right;
}
.dw-masked,
.dw-masked-col {
  color: #6b7688;
  font-style: italic;
}
</style>
