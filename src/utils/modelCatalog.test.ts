/**
 * `utils/modelCatalog.ts` 单元测试（FEAT-165）
 *
 * 这份判定决定「扫描侧模型列表里出不出现创意工坊的模型」——
 * 判反了用户会在扫描设置里看到两个从没用过的超分模型，或者工坊里下不到自己的模型。
 */
import { describe, test } from "node:test";
import assert from "node:assert/strict";

import { isRuntimeComponent, isWorkshopModel, modelsInArea } from "./modelCatalog.ts";

const LIST = [
  { name: "person-det" },
  { name: "local-esrgan" },
  { name: "chinese-clip-fp32" },
  { name: "local-gfpgan" },
  { name: "openvino-runtime" },
  { name: "ocr" },
];

describe("isWorkshopModel", () => {
  test("local- 前缀算创意工坊", () => {
    assert.equal(isWorkshopModel("local-esrgan"), true);
    assert.equal(isWorkshopModel("local-gfpgan"), true);
  });

  test("识别链路模型不算创意工坊", () => {
    assert.equal(isWorkshopModel("person-det"), false);
    assert.equal(isWorkshopModel("chinese-clip-fp32"), false);
    assert.equal(isWorkshopModel("openvino-runtime"), false);
  });
});

describe("isRuntimeComponent", () => {
  test("OpenVINO 运行时是组件不是模型", () => {
    assert.equal(isRuntimeComponent("openvino-runtime"), true);
    assert.equal(isRuntimeComponent("ocr"), false);
  });
});

describe("modelsInArea", () => {
  test("scan 分区只留识别链路模型", () => {
    assert.deepEqual(
      modelsInArea(LIST, "scan").map((d) => d.name),
      ["person-det", "chinese-clip-fp32", "ocr"],
    );
  });

  test("workshop 分区只留本地增强模型", () => {
    assert.deepEqual(
      modelsInArea(LIST, "workshop").map((d) => d.name),
      ["local-esrgan", "local-gfpgan"],
    );
  });

  test("运行时组件在两个分区里都被剔除", () => {
    for (const area of ["scan", "workshop"] as const) {
      assert.equal(
        modelsInArea(LIST, area).some((d) => d.name === "openvino-runtime"),
        false,
      );
    }
  });

  test("空列表不炸，且保持入参顺序", () => {
    assert.deepEqual(modelsInArea([], "scan"), []);
    assert.deepEqual(modelsInArea([{ name: "local-esrgan" }, { name: "ocr" }], "scan"), [
      { name: "ocr" },
    ]);
  });
});
