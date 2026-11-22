/**
 * 模型来源口径（2026-11-22 定调）
 *
 * 规则：**走供应商（OpenAI 格式）的工具按 baseUrl 主机名分流**——
 *   - 回环地址（127.0.0.1 / localhost / ::1）→「API 模型 · 本地」，免费、请求不出本机网络；
 *   - 其它地址 →「API 模型 · 云端」，按调用计费、照片会离开本机。
 *
 * 「本机模型」只留给 catalog 里 `requires.models` 声明的自带推理（如 Real-ESRGAN），
 * 「本机算法」留给不跑模型的纯算法 / 查表（如智能调色）—— 两者与「本地 API」不是一回事：
 * 后者要用户自己把那个本机服务跑起来。
 */
import { describe, test } from "node:test";
import assert from "node:assert/strict";

import { originOfChain, originOfTool } from "./modelOrigin.ts";
import type { AiSettings } from "../types/ai.ts";

/** 一个只绑了「图像编辑」的供应商（人物美颜 / 滤镜迁移都吃这个能力位） */
function withEdit(baseUrl: string, name = "本地网关"): AiSettings {
  return {
    providers: [{ id: "p1", name, baseUrl, apiKey: "", hasKey: false }],
    edit: { providerId: "p1", model: "qwen-image-2.1-turbo" },
  };
}

/** 图像理解 + 图像编辑绑两家（智能修图这类工具两样都要） */
function withVisionAndEdit(visionUrl: string, editUrl: string): AiSettings {
  return {
    providers: [
      { id: "p1", name: "本机理解", baseUrl: visionUrl, apiKey: "", hasKey: false },
      { id: "p2", name: "远端编辑", baseUrl: editUrl, apiKey: "", hasKey: false },
    ],
    vision: { providerId: "p1", model: "qwen-vl-max" },
    edit: { providerId: "p2", model: "qwen-image-edit" },
  };
}

describe("供应商来源按地址分流", () => {
  test("回环 baseUrl → API 模型 · 本地：免费，且不再说「照片不出本机」", () => {
    const info = originOfTool("beauty", withEdit("http://127.0.0.1:8000/v1"));
    assert.equal(info.origin, "api-local");
    assert.equal(info.label, "API 模型 · 本地");
    assert.equal(info.short, "本地 API");
    assert.equal(info.billable, false);
    assert.equal(info.leavesDevice, false);
    assert.match(info.detail, /127\.0\.0\.1/);
    assert.match(info.detail, /免费/);
    assert.doesNotMatch(info.detail, /按调用计费/);
  });

  test("远端 baseUrl → API 模型 · 云端：计费", () => {
    const info = originOfTool(
      "beauty",
      withEdit("https://dashscope.aliyuncs.com/compatible-mode/v1", "阿里百炼"),
    );
    assert.equal(info.origin, "cloud-api");
    assert.equal(info.label, "API 模型 · 云端");
    assert.equal(info.billable, true);
    assert.equal(info.leavesDevice, true);
    assert.match(info.detail, /dashscope\.aliyuncs\.com/);
    assert.match(info.detail, /按调用计费/);
  });

  test("同一条链上本地 + 远端混合 → 整体按云端，并点明本地那几档", () => {
    const info = originOfTool(
      "smart_edit",
      withVisionAndEdit("http://127.0.0.1:11434/v1", "https://gw.example.com/v1"),
    );
    assert.equal(info.origin, "cloud-api");
    assert.match(info.detail, /1 个能力位走本机 API/);
  });

  test("未绑定供应商 → 未配置，且不计费", () => {
    const info = originOfTool("beauty", { providers: [] });
    assert.equal(info.origin, "unconfigured");
    assert.equal(info.billable, false);
    assert.equal(info.leavesDevice, false);
  });

  test("设置尚未载入 → 按云端显示，但不诬告「未配置」", () => {
    const info = originOfTool("beauty", undefined);
    assert.equal(info.origin, "cloud-api");
    assert.match(info.detail, /尚未载入/);
  });
});

describe("本机来源只留给自带模型与纯算法", () => {
  test("画质增强（requires.models: real_esrgan_x4）→ 本机模型", () => {
    assert.equal(originOfTool("upscale").origin, "local-model");
  });

  test("智能调色（不跑模型）→ 本机算法", () => {
    assert.equal(originOfTool("auto_grade").origin, "local-algo");
  });
});

describe("链条聚合", () => {
  test("本地 API 步 + 本机步 → 仍然免费（不摆价格栏）", () => {
    const info = originOfChain(
      [
        { op: "beauty", enabled: true },
        { op: "auto_grade", enabled: true },
      ],
      withEdit("http://127.0.0.1:8000/v1"),
    );
    assert.equal(info.origin, "api-local");
    assert.equal(info.billable, false);
    assert.equal(info.leavesDevice, false);
    assert.match(info.detail, /本机全部免费/);
  });

  test("云端步 + 本机步 → 云端 API（不能说整条链免费）", () => {
    const info = originOfChain(
      [
        { op: "beauty", enabled: true },
        { op: "auto_grade", enabled: true },
      ],
      withEdit("https://dashscope.aliyuncs.com/compatible-mode/v1", "阿里百炼"),
    );
    assert.equal(info.origin, "cloud-api");
    assert.equal(info.leavesDevice, true);
  });

  test("只启用了本机步 → 本机口径", () => {
    const info = originOfChain([{ op: "auto_grade", enabled: true }], withEdit("http://127.0.0.1:8000/v1"));
    assert.equal(info.origin, "local-algo");
    assert.equal(info.billable, false);
  });
});
