<script setup lang="ts">
/**
 * AI 模型设置弹窗（FEAT-152）
 *
 * 多 profile：图像理解 / 图像编辑 / 超分 三个能力，各自可指向不同服务商的模型
 * （baseUrl + apiKey + model 全 OpenAI 兼容格式）。
 * apiKey 由后端加密存储、只回传掩码；此处留空或保持掩码即「沿用旧密钥」。
 */
import { reactive, ref, watch } from "vue";
import { useAiStudioStore } from "../../../stores/aiStudio";
import { useNotify } from "../../../composables/useNotify";
import type { AiProviderKind } from "../../../types/ai";

const open = defineModel<boolean>({ required: true });
const store = useAiStudioStore();
const notify = useNotify();

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
    applySettings();
    notify.success("已保存", "模型配置已加密保存在本机当前账号下");
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
