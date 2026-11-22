<script setup lang="ts">
/**
 * AI 模型设置弹窗（FEAT-152 / FEAT-157 / FEAT-164 / FEAT-165 / FEAT-196）
 *
 * ## FEAT-196：从「三格各填一套地址」改为「供应商库 + 下拉选择」
 *
 * 旧界面是三个能力卡（图像理解 / 图像编辑 / 图像超分），每张卡里手填
 * baseUrl + apiKey + model。真实使用中同一家服务商会被三个能力位复用，于是
 * 同一个地址要填三遍、换 key 要改三处，而且没有任何「可选项」可言。
 *
 * 新界面分两块：
 *   1) 供应商库：每条 = 一个服务商（名称 + baseUrl + apiKey），只存一份；
 *      「快捷填入」按钮保留，作用变成**填充新建供应商的地址**，不再覆盖已存配置。
 *   2) 能力位绑定：每张能力卡里一个**下拉框选供应商** + 一个模型名输入。
 *
 * 模型名留在能力位上而不是供应商上 —— 同一家服务商在「图像理解」用 qwen-vl-max、
 * 在「图像编辑」用 qwen-image-edit 是常态，模型名跟着供应商走会表达不出来。
 *
 * apiKey 由后端加密存储、只回传掩码；此处留空或保持掩码即「沿用旧密钥」。
 *
 * 计费（FEAT-157）：仍是按能力位登记单价（元/张），不随供应商变化而改结构 ——
 * 切换供应商后单价需要用户自己按新服务商的价格改一次。理由：供应商库是
 * 「凭证管理」，计费是「我用多少钱一张」的估算口径，两者强行耦合会让
 * 用户换个本地 Ollama（免费）之后单价仍残留云端价格，反而误导。
 */
import { computed, reactive, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { useAiBilling, AI_MODELS } from "../../../composables/useAiBilling";
import type {
  AiBillingRule,
  AiProviderEntryInput,
  AiProviderKind,
  AiSettingsInput,
} from "../../../types/ai";
import ModelDownloadRows from "../../settings/ModelDownloadRows.vue";
import { hostOf } from "../../../utils/urlHost";

const open = defineModel<boolean>({ required: true });
const store = useAiStudioStore();
const notify = useNotify();
const billing = useAiBilling();

/**
 * 常见 OpenAI 兼容服务：一键填 baseUrl。
 *
 * 只省输入，不决定来源判定：来源按**地址**分流（2026-11-22 定调）——回环地址
 * （Ollama / LM Studio / 本地网关）→「API 模型 · 本地」免费；其它地址 →
 * 「API 模型 · 云端」按调用计费。所以预设名称里不再带「（本机）」，
 * 免得名字和上面三家云端预设看起来是两个体系。
 */
const BASE_PRESETS: { key: string; label: string; baseUrl: string; name: string }[] = [
  { key: "ollama", label: "Ollama", baseUrl: "http://127.0.0.1:11434/v1", name: "Ollama" },
  { key: "lmstudio", label: "LM Studio", baseUrl: "http://127.0.0.1:1234/v1", name: "LM Studio" },
  {
    key: "local-gateway",
    label: "本地图生图网关",
    baseUrl: "http://127.0.0.1:8000/v1",
    name: "本地图生图网关",
  },
  {
    key: "bailian",
    label: "阿里百炼",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    name: "阿里百炼",
  },
];

/* ---------------- 供应商库表单 ---------------- */

interface ProviderRow {
  /** 稳定 key：已存条目用后端 id，新建的用本地临时 key（保存后由后端替换） */
  uid: string;
  /** 后端 id；新建条目为空串 */
  id: string;
  name: string;
  baseUrl: string;
  apiKey: string;
  hasKey: boolean;
  /** 展开编辑（默认收起，列表行只显示名称与地址） */
  expanded: boolean;
  testing: boolean;
  testMsg: string;
  testOk: boolean | null;
}

let tempSeq = 0;
function blankProvider(preset?: { baseUrl: string; name: string }): ProviderRow {
  tempSeq += 1;
  return {
    uid: `tmp-${tempSeq}`,
    id: "",
    name: preset?.name ?? "",
    baseUrl: preset?.baseUrl ?? "",
    apiKey: "",
    hasKey: false,
    expanded: true,
    testing: false,
    testMsg: "",
    testOk: null,
  };
}

const providers = ref<ProviderRow[]>([]);

/** 供应商展示名：自填名称 → baseUrl 的 host → 兜底文案（host 取不到时不显示空白） */
function providerName(p: { name: string; baseUrl: string }): string {
  return p.name.trim() || hostOf(p.baseUrl) || "未命名供应商";
}

/** 下拉框的选项：供应商库 + 一个「未配置」占位 */
const providerOptions = computed(() =>
  providers.value
    .filter((p) => p.baseUrl.trim())
    .map((p) => ({
      uid: p.uid,
      label: providerName(p),
    })),
);

/** 新增供应商：可带一个快捷填入的预设 */
function addProvider(preset?: { baseUrl: string; name: string }) {
  providers.value.push(blankProvider(preset));
}

function removeProvider(uid: string) {
  const idx = providers.value.findIndex((p) => p.uid === uid);
  if (idx < 0) return;
  providers.value.splice(idx, 1);
  // 被删的供应商一旦还被能力位引用，绑定必须同步清空 —— 否则保存后后端会把
  // 这条绑定判为「指向不存在的供应商」而丢弃，用户看到的是「刚选的又没了」。
  for (const k of KINDS) {
    if (bindings[k.key].providerUid === uid) bindings[k.key].providerUid = "";
  }
}

function toggleProvider(uid: string) {
  const p = providers.value.find((x) => x.uid === uid);
  if (p) p.expanded = !p.expanded;
}

/* ---------------- 能力位绑定 ---------------- */

/** 每张能力卡：选哪个供应商（用 uid，未保存的也能选）+ 模型名 */
interface BindingRow {
  providerUid: string;
  model: string;
}

const bindings = reactive<Record<AiProviderKind, BindingRow>>({
  vision: { providerUid: "", model: "" },
  edit: { providerUid: "", model: "" },
  upscale: { providerUid: "", model: "" },
});

const KINDS: { key: AiProviderKind; title: string; need: string; hint: string }[] = [
  {
    key: "vision",
    title: "图像理解",
    need: "主题手账（提炼主题 / 写配文）",
    hint: "如 qwen-vl-max、gpt-4o",
  },
  {
    key: "edit",
    title: "图像编辑",
    need: "手账风格化 / 人物美颜 / 滤镜迁移",
    hint: "如 qwen-image-edit、gpt-image-1",
  },
  {
    key: "upscale",
    title: "图像超分",
    need: "创意组合的云端超分步 / 模型理解上采样（未配置时回落图像编辑）",
    hint: "独立超分模型，可选",
  },
];

/* ---------------- 计费规则（按能力位，结构不变） ---------------- */

const billingForm = reactive<AiBillingRule>({
  currency: "¥",
  modelPrices: { vision: 0, edit: 0, upscale: 0 },
  note: "",
});

const billingOpen = reactive<Record<AiProviderKind, boolean>>({
  vision: false,
  edit: false,
  upscale: false,
});

function toggleBilling(k: AiProviderKind) {
  billingOpen[k] = !billingOpen[k];
}

function applyBilling() {
  const r = billing.rule.value;
  billingForm.currency = r.currency;
  for (const k of AI_MODELS) billingForm.modelPrices[k] = r.modelPrices[k] ?? 0;
  billingForm.note = r.note ?? "";
}

function priceText(k: AiProviderKind): string {
  const v = Number(billingForm.modelPrices[k] ?? 0);
  if (!Number.isFinite(v) || v <= 0) return "未配置";
  return `${billingForm.currency || "¥"}${v}/张`;
}

/* ---------------- 读取 / 保存 ---------------- */

/** 把后端设置铺进表单：先建供应商行（保留它们的 uid），再按 id 找回绑定 */
function applySettings() {
  const s = store.settings;
  const rows: ProviderRow[] = (s?.providers ?? []).map((p, i) => {
    tempSeq += 1;
    return {
      uid: p.id || `tmp-restore-${i}-${tempSeq}`,
      id: p.id,
      name: p.name,
      baseUrl: p.baseUrl,
      apiKey: "",
      hasKey: !!p.hasKey,
      expanded: false,
      testing: false,
      testMsg: "",
      testOk: null,
    };
  });
  providers.value = rows;

  const byId = new Map(rows.map((r) => [r.id, r.uid]));
  for (const k of KINDS) {
    const b = s?.[k.key] ?? null;
    bindings[k.key].providerUid = b?.providerId ? byId.get(b.providerId) ?? "" : "";
    bindings[k.key].model = b?.model ?? "";
  }
}

watch(
  open,
  async (v) => {
    if (!v) return;
    try {
      await store.loadSettings(true);
      applySettings();
      applyBilling();
    } catch (e) {
      notify.error("读取模型设置失败", String(e));
    }
  },
  { immediate: true },
);

const saving = ref(false);

async function save() {
  saving.value = true;
  try {
    const payload: AiSettingsInput = {
      providers: providers.value
        .filter((p) => p.baseUrl.trim())
        .map<AiProviderEntryInput>((p) => ({
          id: p.id,
          name: p.name.trim(),
          baseUrl: p.baseUrl.trim(),
          apiKey: p.apiKey,
        })),
      ...bindingPayload("vision"),
      ...bindingPayload("edit"),
      ...bindingPayload("upscale"),
    };
    await store.saveSettings(payload);
    billing.save({
      currency: billingForm.currency,
      modelPrices: { ...billingForm.modelPrices },
      note: billingForm.note,
    });
    applySettings();
    notify.success(
      "已保存",
      "供应商与密钥已加密保存在本机当前账号下；计费规则已保存在本机",
    );
  } catch (e) {
    notify.error("保存失败", String(e));
  } finally {
    saving.value = false;
  }
}

/**
 * 组装一个能力位的绑定请求。
 *
 * 注意「没选供应商」要显式传 `null` 而不是省略字段：省略字段在后端语义是
 * 「保留旧值」，而用户清空下拉的意图是「解除绑定」。两者混同会让用户改不掉配置。
 */
function bindingPayload(
  k: AiProviderKind,
): Pick<AiSettingsInput, "vision" | "edit" | "upscale"> {
  const uid = bindings[k].providerUid;
  const row = providers.value.find((p) => p.uid === uid);
  const providerId = row?.id ?? "";
  const value =
    row && providerId
      ? { providerId, model: bindings[k].model.trim() }
      : null;
  return { [k]: value } as Pick<AiSettingsInput, "vision" | "edit" | "upscale">;
}

/* ---------------- 测试连通性 ---------------- */

async function testProvider(p: ProviderRow) {
  p.testing = true;
  p.testMsg = "测试中…";
  p.testOk = null;
  try {
    // 测试用「该供应商 + 任一已绑定它的能力位的模型名」。
    // 没有绑定就没有模型名，此时让后端报「请先填写模型名」比乱猜一个模型更诚实。
    const boundModel =
      KINDS.map((k) => (bindings[k.key].providerUid === p.uid ? bindings[k.key].model : ""))
        .find((m) => m.trim()) ?? "";
    const r = await store.testProvider({
      providerId: p.id,
      baseUrl: p.baseUrl,
      model: boundModel,
      apiKey: p.apiKey,
    });
    p.testOk = r.ok;
    p.testMsg = `${r.message}（${r.latencyMs}ms）`;
  } catch (e) {
    p.testOk = false;
    p.testMsg = String(e);
  } finally {
    p.testing = false;
  }
}

/* ---------------- 本地增强模型 / 模型目录 ---------------- */

const modelsDir = ref("");
async function openModelsDir() {
  try {
    if (!modelsDir.value) {
      const { invoke: inv } = await import("@tauri-apps/api/core");
      modelsDir.value = await inv<string>("model_dir");
    }
    if (!modelsDir.value) {
      notify.info("模型目录未知", "下载任一本地模型后可再次尝试；也可在创意工坊的画质修复里查看目录");
      return;
    }
    await invoke("open_folder", { path: modelsDir.value });
  } catch (e) {
    notify.error("打开模型目录失败", String(e));
  }
}
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="asd-mask" role="presentation" @click.self="open = false">
      <div class="asd-dialog" role="dialog" aria-modal="true" aria-label="AI 模型设置">
        <header class="asd-head">
          <h2 class="asd-title">AI 模型设置</h2>
          <button class="asd-close" type="button" aria-label="关闭" @click="open = false">✕</button>
        </header>

        <div class="asd-body">
          <p class="asd-intro">
            先在<strong>供应商库</strong>里登记服务商（OpenAI 兼容格式：baseUrl + apiKey），
            再在下方三个能力位<strong>下拉选择</strong>用哪一家。同一家服务商只需登记一次，
            可被三个能力位共用；模型名跟着能力位走，因为同一家在「图像理解」和「图像编辑」
            通常要用不同模型。baseUrl 填本机地址即走本地服务（Ollama / LM Studio 用于理解类）。
          </p>

          <!-- ══ 供应商库 ══ -->
          <section class="asd-card asd-providers">
            <div class="asd-card-head">
              <h3 class="asd-card-title">供应商库</h3>
              <span class="asd-card-need">{{ providers.length }} 个 · 密钥加密存本机</span>
            </div>

            <div v-if="!providers.length" class="asd-empty">
              还没有登记供应商。点下面「＋ 新增供应商」，或直接用快捷填入建一个。
            </div>

            <div v-for="p in providers" :key="p.uid" class="asd-prov">
              <button class="asd-prov-head" type="button" @click="toggleProvider(p.uid)">
                <span class="asd-prov-caret">{{ p.expanded ? "▾" : "▸" }}</span>
                <span class="asd-prov-name">{{ providerName(p) }}</span>
                <span class="asd-prov-url">{{ p.baseUrl || "（未填地址）" }}</span>
                <span v-if="p.hasKey" class="asd-prov-key" title="已保存密钥">🔑</span>
              </button>

              <div v-if="p.expanded" class="asd-prov-body">
                <div class="asd-grid">
                  <label class="asd-field">
                    <span class="asd-label">名称</span>
                    <input
                      v-model="p.name"
                      class="ai-inp ai-inp-full"
                      :placeholder="hostOf(p.baseUrl) || 'https://…/v1'"
                      spellcheck="false"
                    />
                  </label>
                  <label class="asd-field">
                    <span class="asd-label">baseUrl</span>
                    <input
                      v-model="p.baseUrl"
                      class="ai-inp ai-inp-full"
                      placeholder="https://…/compatible-mode/v1"
                      spellcheck="false"
                    />
                  </label>
                  <label class="asd-field asd-field-wide">
                    <span class="asd-label">apiKey</span>
                    <input
                      v-model="p.apiKey"
                      class="ai-inp ai-inp-full"
                      type="password"
                      autocomplete="off"
                      :placeholder="p.hasKey ? '已保存，留空则不修改' : '粘贴 API Key'"
                      spellcheck="false"
                    />
                  </label>
                </div>

                <div class="asd-prov-foot">
                  <button class="ai-ghost" type="button" :disabled="p.testing" @click="testProvider(p)">
                    {{ p.testing ? "测试中…" : "测试连接" }}
                  </button>
                  <button class="asd-del" type="button" @click="removeProvider(p.uid)">删除</button>
                  <span
                    v-if="p.testMsg"
                    class="asd-test"
                    :class="p.testOk === true ? 'ok' : p.testOk === false ? 'bad' : ''"
                  >
                    {{ p.testMsg }}
                  </span>
                </div>
              </div>
            </div>

            <div class="asd-prov-add">
              <button class="ai-ghost" type="button" @click="addProvider()">＋ 新增供应商</button>
              <span class="asd-presets-label">快捷填入</span>
              <button
                v-for="ps in BASE_PRESETS"
                :key="ps.key"
                type="button"
                class="asd-preset"
                :title="ps.baseUrl"
                @click="addProvider(ps)"
              >
                {{ ps.label }}
              </button>
            </div>
          </section>

          <!-- ══ 计费规则（按能力位，结构不变） ══ -->
          <section class="asd-card asd-billing">
            <div class="asd-card-head">
              <h3 class="asd-card-title">计费规则</h3>
              <span class="asd-card-need">按能力位登记 · 仅本地展示，不代扣费</span>
            </div>
            <p class="asd-billing-note">
              本功能用你自己的 API Key 直连第三方服务商（如阿里云百炼）<strong>按量计费</strong>，
              本项目不经手、不加价。费用按<strong>能力位</strong>登记：在下方每张能力卡片的
              「计费」里展开填写「单价（元/张）」，运行区会按「单价 × 张数」自动预估。
              改用了不同价格的服务商时，请顺手改一次这里。免费额度用尽后接口会返回 403
              （额度或权限不足），需到服务商控制台充值或开通。
            </p>
            <label class="asd-field asd-currency">
              <span class="asd-label">币种符号</span>
              <input v-model="billingForm.currency" class="ai-inp" maxlength="4" placeholder="¥" />
            </label>
          </section>

          <!-- ══ 本地增强模型（FEAT-164）══ -->
          <section class="asd-card asd-local">
            <div class="asd-card-head">
              <h3 class="asd-card-title">本地增强模型</h3>
              <span class="asd-card-need">本机推理 · 免费 · 离线</span>
            </div>
            <p class="asd-billing-note">
              「工具」组的画质修复与 AI 智能修图的本地组件共用这两个模型：应用内直接下载（多镜像 /
              断点续传 / 可取消 / 一键补全），下完即用。不下载也能用胶片模拟与参考图调色
              （内置 LUT + 纯算法，不依赖任何模型）。
            </p>
            <ModelDownloadRows area="workshop" />
            <div class="asd-card-foot">
              <button class="ai-ghost" type="button" @click="openModelsDir">打开模型目录</button>
              <span class="asd-test">与相册识别模型同一模型根（models/local）</span>
            </div>
          </section>

          <!-- ══ 三个能力位：选供应商 + 填模型 ══ -->
          <section v-for="k in KINDS" :key="k.key" class="asd-card">
            <div class="asd-card-head">
              <h3 class="asd-card-title">{{ k.title }}</h3>
              <span class="asd-card-need">{{ k.need }}</span>
            </div>

            <div class="asd-grid">
              <label class="asd-field">
                <span class="asd-label">供应商</span>
                <select v-model="bindings[k.key].providerUid" class="ai-sel ai-inp-full">
                  <option value="">（未配置）</option>
                  <option v-for="o in providerOptions" :key="o.uid" :value="o.uid">
                    {{ o.label }}
                  </option>
                </select>
              </label>

              <label class="asd-field">
                <span class="asd-label">模型</span>
                <input
                  v-model="bindings[k.key].model"
                  class="ai-inp ai-inp-full"
                  :placeholder="k.hint"
                  spellcheck="false"
                />
              </label>
            </div>

            <p v-if="!providerOptions.length" class="asd-hint-inline">
              上面还没有可用供应商 —— 先在上面「供应商库」里新增一个并把 baseUrl 填好。
            </p>

            <div class="asd-billing-sub">
              <button type="button" class="asd-billing-toggle" @click="toggleBilling(k.key)">
                <span class="asd-billing-caret">{{ billingOpen[k.key] ? "▾" : "▸" }}</span>
                <span class="asd-billing-sub-label">计费（元/张）</span>
                <span class="asd-billing-cur">{{ priceText(k.key) }}</span>
              </button>
              <div v-if="billingOpen[k.key]" class="asd-billing-fill">
                <label class="asd-field">
                  <span class="asd-label">{{ k.title }}模型单价（元/张）</span>
                  <input
                    v-model.number="billingForm.modelPrices[k.key]"
                    class="ai-inp ai-inp-full"
                    type="number"
                    min="0"
                    step="0.01"
                    placeholder="0"
                  />
                </label>
                <p class="asd-billing-hint">
                  照该模型官方定价填写；填 0 或留空 = 未配置。参考价以服务商官方为准，可随时修改。
                </p>
              </div>
            </div>
          </section>
        </div>

        <footer class="asd-foot">
          <p class="ai-privacy" style="flex: 1">
            apiKey 使用本机应用主密钥加密后保存在当前账号下，仅在你点击生成时经本机回环传给 AI 创作服务，不会写入日志或上传。
          </p>
          <button class="ai-ghost" type="button" @click="open = false">取消</button>
          <button class="ai-run" type="button" :disabled="saving" @click="save">
            {{ saving ? "保存中…" : "保存" }}
          </button>
        </footer>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.asd-mask {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(16, 24, 40, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
}

.asd-dialog {
  width: min(720px, 96vw);
  max-height: min(88vh, 900px);
  display: flex;
  flex-direction: column;
  border-radius: var(--radius-lg, 14px);
  background: var(--color-surface, #1c1b22);
  box-shadow: 0 20px 60px rgba(16, 24, 40, 0.32);
  overflow: hidden;
}

.asd-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid var(--c-rule);
}
.asd-title {
  margin: 0;
  font-size: var(--fs-head);
  font-weight: var(--fw-strong);
  color: var(--fg);
}
.asd-close {
  border: none;
  background: none;
  color: var(--fg-2);
  font-size: 15px;
  cursor: pointer;
  padding: 4px 8px;
  border-radius: 6px;
}
.asd-close:hover {
  color: var(--fg);
  background: var(--hover);
}

.asd-body {
  padding: 16px 20px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.asd-intro {
  margin: 0;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.6;
}

.asd-card {
  border: 1px solid var(--c-rule);
  border-radius: var(--r-ui);
  padding: 12px 14px 10px;
  background: var(--hover, rgba(120, 120, 130, 0.05));
}

.asd-card-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 10px;
}
.asd-card-title {
  margin: 0;
  font-size: var(--fs-title);
  font-weight: var(--fw-em);
  color: var(--fg);
}
.asd-card-need {
  font-size: var(--fs-meta);
  color: var(--fg-2);
}

/* ---------- 供应商库 ---------- */
.asd-providers {
  border-color: color-mix(in srgb, var(--c-accent) 32%, var(--c-rule));
}
.asd-empty {
  margin: 2px 0 10px;
  font-size: var(--fs-meta);
  color: var(--fg-3);
  line-height: 1.6;
}
.asd-prov {
  border: 1px solid var(--c-rule);
  border-radius: var(--r-ui);
  margin-bottom: 8px;
  background: color-mix(in srgb, var(--fg) 3%, transparent);
}
.asd-prov-head {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 7px 10px;
  border: none;
  background: none;
  color: var(--fg);
  font-size: var(--fs-body);
  text-align: left;
  cursor: pointer;
}
.asd-prov-head:hover {
  background: var(--hover);
}
.asd-prov-caret {
  width: 1em;
  flex: none;
  color: var(--c-accent);
}
.asd-prov-name {
  flex: none;
  font-weight: var(--fw-em);
  max-width: 40%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.asd-prov-url {
  flex: 1;
  min-width: 0;
  font-size: var(--fs-meta);
  color: var(--fg-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.asd-prov-key {
  flex: none;
  font-size: 11px;
}
.asd-prov-body {
  padding: 4px 10px 10px;
  border-top: 1px dashed var(--c-rule);
}
.asd-prov-foot {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  margin-top: 8px;
}
.asd-del {
  height: 30px;
  padding: 0 10px;
  border: 1px solid var(--c-rule);
  border-radius: 7px;
  background: none;
  color: var(--c-danger);
  font-size: var(--fs-meta);
  cursor: pointer;
}
.asd-del:hover {
  border-color: var(--c-danger);
  background: color-mix(in srgb, var(--c-danger) 10%, transparent);
}
.asd-prov-add {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  margin-top: 4px;
}
.asd-presets-label {
  margin-left: 4px;
  font-size: var(--fs-label);
  letter-spacing: 0.12em;
  color: var(--fg-3);
}
.asd-preset {
  height: 22px;
  padding: 0 8px;
  border: none;
  border-radius: var(--r-pill);
  box-shadow: inset 0 0 0 1px var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: 11px;
  cursor: pointer;
}
.asd-preset:hover {
  color: var(--fg);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 45%, transparent);
}

/* 计费规则（按能力位登记） */
.asd-billing {
  border-color: color-mix(in srgb, var(--c-accent) 32%, var(--c-rule));
}
/* 本地模型卡：与计费卡同等的强调（两者都是"设置里的独立子域"） */
.asd-local {
  border-color: color-mix(in srgb, var(--c-accent) 24%, var(--c-rule));
}

.asd-billing-note {
  margin: 0 0 10px;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.6;
}
.asd-currency {
  max-width: 160px;
}
.asd-billing-sub {
  margin-top: 10px;
  border-top: 1px dashed var(--c-rule);
  padding-top: 8px;
}
.asd-billing-toggle {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 4px 2px;
  border: none;
  background: none;
  color: var(--fg-2);
  font-size: var(--fs-meta);
  cursor: pointer;
  text-align: left;
}
.asd-billing-toggle:hover {
  color: var(--fg);
}
.asd-billing-caret {
  width: 1em;
  color: var(--c-accent);
}
.asd-billing-sub-label {
  font-weight: var(--fw-em);
}
.asd-billing-cur {
  margin-left: auto;
  color: var(--fg-2);
  font-variant-numeric: tabular-nums;
}
.asd-billing-fill {
  padding: 6px 2px 2px;
}
.asd-billing-hint {
  margin: 6px 0 0;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  opacity: 0.85;
  line-height: 1.5;
}

.asd-hint-inline {
  margin: 8px 0 0;
  font-size: var(--fs-meta);
  color: var(--fg-3);
  line-height: 1.5;
}

.asd-grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: 10px 14px;
}
.asd-field {
  display: flex;
  flex-direction: column;
  gap: 5px;
  min-width: 0;
}
.asd-field-wide {
  grid-column: 1 / -1;
}
.asd-label {
  font-size: var(--fs-label);
  letter-spacing: 0.12em;
  color: var(--fg-2);
}

.asd-card-foot {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 10px;
  flex-wrap: wrap;
}
.asd-test {
  font-size: var(--fs-meta);
  color: var(--fg-2);
}
.asd-test.ok {
  color: var(--c-accent);
}
.asd-test.bad {
  color: var(--c-danger);
}

.asd-foot {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 14px 20px;
  border-top: 1px solid var(--c-rule);
  flex-wrap: wrap;
}

@media (max-width: 620px) {
  .asd-grid {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>
