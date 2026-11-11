# -*- coding: utf-8 -*-
"""向 feature-notes.json 追加一条功能登记（FEAT-156）。读-改-写，写后校验。"""
import json, io, sys

PATH = "feature-notes.json"
NEW_ID = "FEAT-156"

entry = {
    "id": NEW_ID,
    "name": "创意工坊 · AI 创作前端「计费规则」与运行前费用预估",
    "module": "src/types/ai.ts、src/composables/useAiBilling.ts（新增）、src/components/workshop/ai/AiSettingsDialog.vue、src/components/workshop/ai/AiCostHint.vue（新增）、src/components/workshop/ai/AiResultPanel.vue、src/components/workshop/ai/{BeautyTool,JournalTool,StyleTransferTool,UpscaleTool}.vue、src/assets/ai-tool.css",
    "status": "done",
    "date": "2026-11-11",
    "purpose": "用户额度用完后提出「前端加一个计费规则」。在 AI 模型设置里可配置每个工具每张单价（本机保存），四个 AI 工具运行前按所选张数显示预估金额与调用次数；额度/权限不足时给出到服务商控制台充值或开通的引导。纯前端展示层：不接支付、不代扣费、不改后端作业流程。",
    "implementation": {
        "分层": "useAiBilling.ts 负责规则读写（localStorage 键 aiStudio:billing:v1）、unitPrice/estimate/formatMoney 与 isQuotaError；AiCostHint.vue 复用在四个工具运行区；AiSettingsDialog.vue 新增「计费规则」区块，点保存时与模型配置一并落盘（计费走 localStorage，与账号设置解耦，无需改 Rust）。",
        "计费口径": "按工具四个单价（元/张），默认 0 或留空 = 未配置；预估 = 张数 × 单价，文案统一注明「预估，实际以服务商账单为准」；币种默认 ¥ 可改。",
        "额度错误引导": "AiResultPanel 用 isQuotaError 识别 insufficient_quota / AccessDenied.Unpurchased / 未开通 / 无使用权限 / 额度 等关键字，附「去服务商控制台充值或开通」提示，而不是只显示一串 403。",
    },
    "highlights": "不改后端、不动作业流程、不接支付；单价可配置、默认未配置时不臆造价格；规则为模块级共享状态，设置改完即时反映到各运行区；纯前端 localStorage，零数据迁移风险。",
    "tests": "npx vue-tsc --noEmit 0 错误；npm run build（vue-tsc --noEmit && vite build）通过。",
    "related_files": [
        "src/types/ai.ts",
        "src/composables/useAiBilling.ts",
        "src/components/workshop/ai/AiSettingsDialog.vue",
        "src/components/workshop/ai/AiCostHint.vue",
        "src/components/workshop/ai/AiResultPanel.vue",
        "src/components/workshop/ai/BeautyTool.vue",
        "src/components/workshop/ai/JournalTool.vue",
        "src/components/workshop/ai/StyleTransferTool.vue",
        "src/components/workshop/ai/UpscaleTool.vue",
        "src/assets/ai-tool.css",
    ],
    "keywords": ["计费规则", "费用预估", "单价", "额度", "insufficient_quota", "AccessDenied.Unpurchased", "localStorage", "useAiBilling", "AiCostHint"],
    "created_at": "2026-11-11T14:45:00+08:00",
    "updated_at": "2026-11-11T14:45:00+08:00",
}

with io.open(PATH, "r", encoding="utf-8") as f:
    data = json.load(f)

ids = [feat.get("id") for feat in data["features"]]
if NEW_ID in ids:
    print("SKIP: %s already exists" % NEW_ID)
    sys.exit(0)

data["features"].append(entry)
data["updated"] = "2026-11-11T14:45:00+08:00"

with io.open(PATH, "w", encoding="utf-8", newline="\n") as f:
    json.dump(data, f, ensure_ascii=False, indent=1)
    f.write("\n")

# 校验
with io.open(PATH, "r", encoding="utf-8") as f:
    check = json.load(f)
print("OK features=%d last=%s" % (len(check["features"]), check["features"][-1]["id"]))
