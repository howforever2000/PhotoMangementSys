/**
 * `composables/useAiBilling.ts` 单元测试（FEAT-152 / FEAT-157）
 *
 * 钉死计费预估最容易出错的几点：
 *   1. 未配置单价时必须是「未配置」而不是 0 元账单；
 *   2. 费用按「工具 → 所用模型 → 该模型单价 × 张数」预估（张数取整、负数/非法按 0）；
 *   3. 旧版「按工具单价」(v1) 能自动迁移到「按模型」(v2)，不丢数据；
 *   4. formatMoney 去掉多余尾零；额度/权限类错误能识别、普通网络错误不误报。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { useAiBilling, isQuotaError } from "./useAiBilling.ts";
import type { AiBillingRule } from "../types/ai.ts";

test("默认未配置单价：不显示金额，total=0 且 configured=false", () => {
  const { rule, estimate } = useAiBilling();
  // 还原默认（避免受其它用例或本机 localStorage 影响）
  rule.value = { currency: "¥", modelPrices: { vision: 0, edit: 0, upscale: 0 } };
  const e = estimate("beauty", 3);
  assert.equal(e.configured, false);
  assert.equal(e.total, 0);
  assert.equal(e.unit, 0);
  assert.equal(e.model, "edit");
});

test("按模型单价预估：美颜/滤镜/手账取编辑模型，画质增强取超分模型", () => {
  const { save, estimate } = useAiBilling();
  const saved = save({ currency: "¥", modelPrices: { vision: 1, edit: 0.2, upscale: 0.5 } });
  assert.equal(saved.modelPrices.edit, 0.2);

  const beauty = estimate("beauty", 3);
  assert.equal(beauty.configured, true);
  assert.equal(beauty.model, "edit");
  assert.ok(Math.abs(beauty.total - 0.6) < 1e-9);

  const style = estimate("style_transfer", 2);
  assert.equal(style.model, "edit");
  assert.ok(Math.abs(style.total - 0.4) < 1e-9);

  const journal = estimate("journal", 2);
  assert.equal(journal.model, "edit"); // 手账只计图像编辑出图成本
  assert.ok(Math.abs(journal.total - 0.4) < 1e-9);

  const up = estimate("upscale", 2);
  assert.equal(up.model, "upscale");
  assert.ok(Math.abs(up.total - 1.0) < 1e-9);

  const odd = estimate("beauty", 2.9);
  assert.equal(odd.count, 2); // 向下取整
  const neg = estimate("beauty", -5);
  assert.equal(neg.count, 0);
});

test("旧版按工具单价自动迁移到按模型", () => {
  const { save, estimate } = useAiBilling();
  // 传入旧版结构（含 unitPrices），normalize 应迁移为 modelPrices
  const legacy = {
    currency: "¥",
    unitPrices: { journal: 0, beauty: 0.2, style_transfer: 0.25, upscale: 0.5 },
  } as unknown as AiBillingRule;
  const migrated = save(legacy);
  assert.equal(migrated.modelPrices.edit, 0.2); // 取 beauty 非零价
  assert.equal(migrated.modelPrices.upscale, 0.5);
  assert.equal(migrated.modelPrices.vision, 0);

  const up = estimate("upscale", 2);
  assert.ok(Math.abs(up.total - 1.0) < 1e-9);
});

test("formatMoney 去尾零", () => {
  const { formatMoney } = useAiBilling();
  assert.equal(formatMoney(0.6, "¥"), "¥0.6");
  assert.equal(formatMoney(12, "¥"), "¥12");
  assert.equal(formatMoney(0.055, "$"), "$0.055");
});

test("isQuotaError 识别额度/权限不足，不误伤普通错误", () => {
  assert.equal(isQuotaError("403 insufficient_quota: Free quota exhausted"), true);
  assert.equal(isQuotaError("AccessDenied.Unpurchased"), true);
  assert.equal(isQuotaError("额度或权限不足"), true);
  assert.equal(isQuotaError("网络连接被重置 10054"), false);
  assert.equal(isQuotaError(""), false);
  assert.equal(isQuotaError(null), false);
});
