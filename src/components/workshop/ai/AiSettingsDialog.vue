<script setup lang="ts">
/**
 * AI 模型设置弹窗（FEAT-152 / FEAT-157）
 *
 * 多 profile：图像理解 / 图像编辑 / 超分 三个能力，各自可指向不同服务商的模型
 * （baseUrl + apiKey + model 全 OpenAI 兼容格式）。
 * apiKey 由后端加密存储、只回传掩码；此处留空或保持掩码即「沿用旧密钥」。
 *
 * 计费（FEAT-157）：从「按工具」改为「按模型」登记——每个模型卡片内可展开填写
 * 该模型的单价（元/张），同一模型被多个工具复用时只填一次。规则仍只存本机
 * localStorage，仅用于运行前的费用预估展示，不代扣费、不改后端。
 */
import { reactive, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import { useAiBilling, AI_MODELS } from "../../../composables/useAiBilling";
import type { AiBillingRule, AiProviderKind } from "../../../types/ai";

const open = defineModel<boolean>({ required: true });
const store = useAiStudioStore();
const notify = useNotify();
const billing = useAiBilling();

/** 计费规则表单（本地保存，仅用于运行前的费用预估展示；按模型登记） */
const billingForm = reactive<AiBillingRule>({
  currency: "¥",
  modelPrices: { vision: 0, edit: 0, upscale: 0 },
  note: "",
});

/** 每个模型卡片内的「计费」折叠状态（默认收起，展开才填写） */
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

/** 折叠条上显示当前单价文案 */
function priceText(k: AiProviderKind): string {
  const v = Number(billingForm.modelPrices[k] ?? 0);
  if (!Number.isFinite(v) || v <= 0) return "未配置";
  return `${billingForm.currency || "¥"}${v}/张`;
}

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
    need: "画质增强（未配置时回落图像编辑）",
    hint: "独立超分模型，可选",
  },
];

interface Row {
  baseUrl: string;
  apiKey: string;
  model: string;
  hasKey: boolean;
  testing: boolean;
  testMsg: string;
  testOk: boolean | null;
}

function blank(): Row {
  return { baseUrl: "", apiKey: "", model: "", hasKey: false, testing: false, testMsg: "", testOk: null };
}

const form = reactive<Record<AiProviderKind, Row>>({
  vision: blank(),
  edit: blank(),
  upscale: blank(),
});

function applySettings() {
  const s = store.settings;
  for (const k of KINDS) {
    const p = s?.providers?.[k.key] ?? null;
    form[k.key].baseUrl = p?.baseUrl ?? "";
    form[k.key].apiKey = "";
    form[k.key].model = p?.model ?? "";
    form[k.key].hasKey = !!p?.hasKey;
    form[k.key].testMsg = "";
    form[k.key].testOk = null;
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
    const pick = (k: AiProviderKind) => ({
      baseUrl: form[k].baseUrl,
      apiKey: form[k].apiKey,
      model: form[k].model,
    });
    await store.saveSettings({
      providers: {
        vision: pick("vision"),
        edit: pick("edit"),
        upscale: pick("upscale"),
      },
    });
    billing.save({
      currency: billingForm.currency,
      modelPrices: { ...billingForm.modelPrices },
      note: billingForm.note,
    });
    applySettings();
    notify.success("已保存", "模型配置已加密保存在本机当前账号下；计费规则已保存在本机");
  } catch (e) {
    notify.error("保存失败", String(e));
  } finally {
    saving.value = false;
  }
}

async function test(kind: AiProviderKind) {
  const f = form[kind];
  f.testing = true;
  f.testMsg = "测试中…";
  f.testOk = null;
  try {
    const r = await store.testProvider(kind, {
      baseUrl: f.baseUrl,
      apiKey: f.apiKey,
      model: f.model,
    });
    f.testOk = r.ok;
    f.testMsg = `${r.message}（${r.latencyMs}ms）`;
  } catch (e) {
    f.testOk = false;
    f.testMsg = String(e);
  } finally {
    f.testing = false;
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
            三个能力各自填写一个 <strong>OpenAI 兼容</strong> 服务商的 baseUrl / apiKey / 模型名。
            可指向同一服务商的不同模型，也可分别指向不同服务商。
          </p>

          <section class="asd-card asd-billing">
            <div class="asd-card-head">
              <h3 class="asd-card-title">计费规则</h3>
              <span class="asd-card-need">按模型登记 · 仅本地展示，不代扣费</span>
            </div>
            <p class="asd-billing-note">
              本功能用你自己的 API Key 直连第三方服务商（如阿里云百炼）<strong>按量计费</strong>，本项目不经手、不加价。
              费用按<strong>模型</strong>登记：在下方每个模型卡片的「计费」里展开填写该模型的「单价（元/张）」，
              同一模型被多个工具复用时只需填一次；运行区会按「所用模型单价 × 张数」自动预估。
              单价请照你所用模型的官方定价填写。免费额度用尽后接口会返回 403（额度或权限不足），需到服务商控制台充值或开通。
            </p>
            <label class="asd-field asd-currency">
              <span class="asd-label">币种符号</span>
              <input v-model="billingForm.currency" class="ai-inp" maxlength="4" placeholder="¥" />
            </label>
          </section>

          <section v-for="k in KINDS" :key="k.key" class="asd-card">
            <div class="asd-card-head">
              <h3 class="asd-card-title">{{ k.title }}</h3>
              <span class="asd-card-need">{{ k.need }}</span>
            </div>

            <div class="asd-grid">
              <label class="asd-field">
                <span class="asd-label">baseUrl</span>
                <input
                  v-model="form[k.key].baseUrl"
                  class="ai-inp ai-inp-full"
                  placeholder="https://…/compatible-mode/v1"
                  spellcheck="false"
                />
              </label>

              <label class="asd-field">
                <span class="asd-label">模型</span>
                <input
                  v-model="form[k.key].model"
                  class="ai-inp ai-inp-full"
                  :placeholder="k.hint"
                  spellcheck="false"
                />
              </label>

              <label class="asd-field asd-field-wide">
                <span class="asd-label">apiKey</span>
                <input
                  v-model="form[k.key].apiKey"
                  class="ai-inp ai-inp-full"
                  type="password"
                  autocomplete="off"
                  :placeholder="form[k.key].hasKey ? '已保存，留空则不修改' : '粘贴 API Key'"
                  spellcheck="false"
                />
              </label>
            </div>

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

            <div class="asd-card-foot">
              <button class="ai-ghost" type="button" :disabled="form[k.key].testing" @click="test(k.key)">
                {{ form[k.key].testing ? "测试中…" : "测试连接" }}
              </button>
              <span
                v-if="form[k.key].testMsg"
                class="asd-test"
                :class="form[k.key].testOk === true ? 'ok' : form[k.key].testOk === false ? 'bad' : ''"
              >
                {{ form[k.key].testMsg }}
              </span>
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

/* 计费规则（feat: 按模型登记） */
.asd-billing {
  border-color: color-mix(in srgb, var(--c-accent) 32%, var(--c-rule));
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
