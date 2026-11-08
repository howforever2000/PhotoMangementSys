<script setup lang="ts">
/**
 * 模型服务设置：端点 CRUD + 连通性探针。
 *
 * 为什么放在工坊里而不是跑到"设置页"：用户在哪儿卡住就在哪儿解决。
 * 选了「AI 插画化」发现没配端点，就地展开填，不必去找设置入口。
 *
 * 两条纪律（dev-memory 交互标准）：
 * - 保存/测试按钮点下即 disabled，防重复提交；
 * - 测试结果逐项列出（参数/模型列表/对话端点/出图），失败给出**下次该改什么**，
 *   而不是一句"连接失败"。
 */
import { computed, ref } from "vue";
import { useAgentStore } from "../../../stores/agent";
import type { ProbeReport, ProviderInput } from "../../../types/agent";

const store = useAgentStore();

const open = ref(false);
const editing = ref(false);
const saving = ref(false);
const testing = ref(false);
const error = ref("");
const report = ref<ProbeReport | null>(null);

function emptyInput(): ProviderInput {
  return {
    label: "",
    baseUrl: "",
    model: "",
    apiKey: "",
    unitPrice: 0,
    kind: "image",
    asyncPoll: false,
  };
}

const form = ref<ProviderInput>(emptyInput());
const editingId = ref<string>("");

const hasImage = computed(() => store.providers.some((p) => p.kind === "image" || p.kind === "both"));
const hasPlanner = computed(() =>
  store.providers.some((p) => p.kind === "planner" || p.kind === "both"),
);

function startAdd() {
  form.value = emptyInput();
  editingId.value = "";
  editing.value = true;
  error.value = "";
  report.value = null;
}

function startEdit(id: string) {
  const p = store.providers.find((x) => x.id === id);
  if (!p) return;
  form.value = {
    id: p.id,
    label: p.label,
    baseUrl: p.baseUrl,
    model: p.model,
    // 前端拿不到明文 Key：留空 = 沿用
    apiKey: "",
    unitPrice: p.unitPrice,
    kind: p.kind,
    asyncPoll: p.asyncPoll,
  };
  editingId.value = p.id;
  editing.value = true;
  error.value = "";
  report.value = null;
}

async function save() {
  if (saving.value) return;
  saving.value = true;
  error.value = "";
  try {
    const payload: ProviderInput = { ...form.value };
    if (editingId.value) payload.id = editingId.value;
    await store.saveProvider(payload);
    editing.value = false;
    // 保存成功后立刻测一次：用户下一步一定是"这玩意通不通"
    if (payload.baseUrl.trim()) {
      await test();
    }
  } catch (e) {
    error.value = typeof e === "string" ? e : String(e);
  } finally {
    saving.value = false;
  }
}

async function test() {
  if (testing.value) return;
  testing.value = true;
  report.value = null;
  error.value = "";
  try {
    const payload: ProviderInput = { ...form.value };
    if (editingId.value) payload.id = editingId.value;
    report.value = await store.testProvider(payload);
  } catch (e) {
    error.value = typeof e === "string" ? e : String(e);
  } finally {
    testing.value = false;
  }
}

async function remove(id: string) {
  try {
    await store.deleteProvider(id);
    if (editingId.value === id) editing.value = false;
  } catch (e) {
    error.value = typeof e === "string" ? e : String(e);
  }
}

function kindLabel(k: string): string {
  return k === "image" ? "出图" : k === "planner" ? "规划" : "两者都行";
}
</script>

<template>
  <details class="pp" :open="open" @toggle="open = ($event.target as HTMLDetailsElement).open">
    <summary class="pp-sum">
      模型服务
      <span class="pp-badge" :class="{ ok: hasImage && hasPlanner, warn: !hasImage || !hasPlanner }">
        {{ store.providers.length === 0 ? "未配置" : `${store.providers.length} 个端点` }}
      </span>
    </summary>

    <p class="pp-hint">
      只在需要模型时才用得到。本地算法预设（压缩/调色/人像/卡片）不需要它们，也不花钱。
      地址填 OpenAI 兼容根路径，例如 <code>https://host/v1</code> 或 <code>http://127.0.0.1:11434/v1</code>。
    </p>

    <ul v-if="store.providers.length" class="pp-list">
      <li v-for="p in store.providers" :key="p.id" class="pp-item">
        <span class="pp-name">{{ p.label }}</span>
        <span class="pp-tag">{{ kindLabel(p.kind) }}</span>
        <span class="pp-meta">{{ p.model }}</span>
        <span v-if="p.unitPrice > 0" class="pp-meta">¥{{ p.unitPrice }}/次</span>
        <span class="pp-key">{{ p.hasKey ? p.keyHint : "无 Key（本地）" }}</span>
        <span class="pp-actions">
          <button type="button" class="pp-btn" @click="startEdit(p.id)">编辑</button>
          <button type="button" class="pp-btn" @click="remove(p.id)">删除</button>
        </span>
      </li>
    </ul>
    <p v-else class="pp-empty">还没有配置端点。需要 AI 重绘 / 规划时再加。</p>

    <button v-if="!editing" type="button" class="pp-add" @click="startAdd">+ 添加端点</button>

    <div v-if="editing" class="pp-form">
      <label class="pp-row">
        <span class="pp-k">地址</span>
        <input v-model="form.baseUrl" class="pp-input" placeholder="https://host/v1" />
      </label>
      <label class="pp-row">
        <span class="pp-k">模型名</span>
        <input v-model="form.model" class="pp-input" placeholder="按服务商文档填，如 seedream-5-0-pro" />
      </label>
      <label class="pp-row">
        <span class="pp-k">API Key</span>
        <input
          v-model="form.apiKey"
          class="pp-input"
          type="password"
          :placeholder="editingId ? '留空 = 沿用已保存的 Key' : '公网端点必填；本地可留空'"
        />
      </label>
      <label class="pp-row">
        <span class="pp-k">用途</span>
        <select v-model="form.kind" class="pp-input">
          <option value="image">出图</option>
          <option value="planner">规划</option>
          <option value="both">两者都行</option>
        </select>
      </label>
      <label class="pp-row">
        <span class="pp-k">单价</span>
        <input v-model.number="form.unitPrice" class="pp-input pp-num" type="number" min="0" step="0.01" />
        <span class="pp-unit">元/次（未知填 0）</span>
      </label>
      <label class="pp-check">
        <input v-model="form.asyncPoll" type="checkbox" />
        <span>异步任务（提交后轮询；Seedream 这类需要）</span>
      </label>

      <div class="pp-btns">
        <button type="button" class="pp-primary" :disabled="saving" @click="save">
          {{ saving ? "保存中…" : "保存" }}
        </button>
        <button type="button" class="pp-btn" :disabled="testing" @click="test">
          {{ testing ? "测试中…" : "测试连接" }}
        </button>
        <button type="button" class="pp-btn" @click="editing = false">取消</button>
      </div>

      <p v-if="error" class="pp-error">{{ error }}</p>

      <div v-if="report" class="pp-report">
        <p class="pp-report-head" :class="{ ok: report.ok }">
          {{ report.ok ? "连接可用" : "连接有问题" }}
        </p>
        <ul>
          <li v-for="c in report.checks" :key="c.name" :class="{ bad: !c.ok }">
            <span class="pp-dot" :class="{ ok: c.ok }" />
            <b>{{ c.name }}</b>
            <span v-if="c.ms" class="pp-ms">{{ c.ms }}ms</span>
            <span class="pp-detail">{{ c.detail }}</span>
          </li>
        </ul>
        <p v-if="report.models.length" class="pp-models">
          可用模型：{{ report.models.slice(0, 8).join("、") }}{{ report.models.length > 8 ? " …" : "" }}
        </p>
      </div>
    </div>
  </details>
</template>

<style scoped>
.pp {
  font-size: 11.5px;
  color: var(--fg-2, #a8a29a);
}

.pp-sum {
  cursor: pointer;
  font-size: 11px;
  letter-spacing: 0.04em;
}

.pp-badge {
  margin-left: 6px;
  padding: 1px 6px;
  border-radius: 9px;
  font-size: 10px;
  border: 1px solid var(--line, rgba(255, 255, 255, 0.16));
}

.pp-badge.ok {
  color: #4caf7d;
  border-color: rgba(76, 175, 125, 0.5);
}

.pp-badge.warn {
  color: var(--gold, #c8a45c);
  border-color: rgba(200, 164, 92, 0.45);
}

.pp-hint {
  margin: 7px 0;
  line-height: 1.6;
}

.pp-hint code {
  padding: 0 3px;
  border-radius: 3px;
  background: var(--field, rgba(255, 255, 255, 0.06));
  font-size: 10.5px;
}

.pp-list {
  margin: 0 0 7px;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.pp-item {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  padding: 5px 7px;
  border: 1px solid var(--line, rgba(255, 255, 255, 0.1));
  border-radius: 6px;
}

.pp-name {
  color: var(--fg, #ece7df);
  font-weight: 600;
}

.pp-tag {
  padding: 0 5px;
  border-radius: 4px;
  background: var(--field, rgba(255, 255, 255, 0.06));
  font-size: 10px;
}

.pp-meta,
.pp-key {
  font-size: 10.5px;
  color: var(--fg-3, #6f6a63);
}

.pp-actions {
  margin-left: auto;
  display: flex;
  gap: 4px;
}

.pp-btn,
.pp-primary,
.pp-add {
  padding: 3px 9px;
  border: 1px solid var(--line, rgba(255, 255, 255, 0.16));
  border-radius: 5px;
  background: none;
  color: var(--fg-2, #a8a29a);
  font-size: 11px;
  cursor: pointer;
}

.pp-primary {
  border-color: var(--gold, #c8a45c);
  background: var(--gold, #c8a45c);
  color: #1b1713;
  font-weight: 600;
}

.pp-primary:disabled,
.pp-btn:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.pp-add {
  margin-top: 2px;
}

.pp-empty {
  margin: 0 0 7px;
}

.pp-form {
  margin-top: 8px;
  padding: 9px;
  border: 1px solid var(--line, rgba(255, 255, 255, 0.12));
  border-radius: 8px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.pp-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.pp-k {
  flex: none;
  width: 52px;
  font-size: 11px;
  color: var(--fg-2, #a8a29a);
}

.pp-input {
  flex: 1;
  min-width: 0;
  padding: 4px 7px;
  border: 1px solid var(--line, rgba(255, 255, 255, 0.14));
  border-radius: 5px;
  background: var(--field, rgba(255, 255, 255, 0.04));
  color: var(--fg, #ece7df);
  font-size: 11.5px;
  font-family: inherit;
}

.pp-num {
  flex: none;
  width: 80px;
}

.pp-unit {
  font-size: 10.5px;
  color: var(--fg-3, #6f6a63);
}

.pp-check {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
}

.pp-btns {
  display: flex;
  gap: 6px;
  margin-top: 2px;
}

.pp-error {
  margin: 0;
  color: #d9534f;
  font-size: 11px;
  line-height: 1.5;
}

.pp-report {
  margin-top: 6px;
  padding-top: 6px;
  border-top: 1px solid var(--line, rgba(255, 255, 255, 0.1));
}

.pp-report-head {
  margin: 0 0 4px;
  font-weight: 600;
  color: #d9534f;
}

.pp-report-head.ok {
  color: #4caf7d;
}

.pp-report ul {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.pp-report li {
  display: flex;
  align-items: baseline;
  gap: 5px;
  flex-wrap: wrap;
  font-size: 10.5px;
}

.pp-report li.bad b {
  color: #d9534f;
}

.pp-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: #d9534f;
  flex: none;
}

.pp-dot.ok {
  background: #4caf7d;
}

.pp-ms {
  color: var(--fg-3, #6f6a63);
  font-variant-numeric: tabular-nums;
}

.pp-detail {
  flex-basis: 100%;
  padding-left: 10px;
  color: var(--fg-2, #a8a29a);
  line-height: 1.5;
}

.pp-models {
  margin: 5px 0 0;
  font-size: 10.5px;
  color: var(--fg-3, #6f6a63);
  line-height: 1.5;
}
</style>
