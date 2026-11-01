/**
 * `utils/bytes.ts` / `utils/pathParts.ts` 单元测试（FEAT-150）
 *
 * 这两组函数一起决定用户看到的「省了多少」与「输出到哪」——
 * 算错一次，用户就会以为压缩没生效，或者找不到产物。
 */
import { describe, test } from "node:test";
import assert from "node:assert/strict";

import { formatBytes, savedPercent } from "./bytes.ts";
import { baseName, dirName, joinPath } from "./pathParts.ts";

describe("formatBytes", () => {
  test("小于 1MB 用 KB 整数", () => {
    assert.equal(formatBytes(480 * 1024), "480 KB");
  });

  test("MB 保留一位小数", () => {
    assert.equal(formatBytes(2.4 * 1024 * 1024), "2.4 MB");
  });

  test("GB 边界按 1024 进一位", () => {
    assert.equal(formatBytes(1024 * 1024 * 1024), "1.0 GB");
  });

  test("0 与非法值都归零成 0 KB", () => {
    assert.equal(formatBytes(0), "0 KB");
    assert.equal(formatBytes(-1), "0 KB");
    assert.equal(formatBytes(Number.NaN), "0 KB");
  });
});

describe("savedPercent", () => {
  test("正常压缩给出减少百分比", () => {
    assert.equal(savedPercent(1000, 180), "体积减少 82%");
  });

  test("总体积为 0 时不编造结果", () => {
    assert.equal(savedPercent(0, 0), "");
  });

  test("输出反而更大时如实说未减少", () => {
    assert.equal(savedPercent(100, 120), "体积未减少");
  });
});

describe("路径拆分", () => {
  test("Windows 反斜杠", () => {
    assert.equal(baseName("D:\\pics\\a.jpg"), "a.jpg");
    assert.equal(dirName("D:\\pics\\a.jpg"), "D:\\pics");
  });

  test("POSIX 斜杠", () => {
    assert.equal(baseName("/pics/a.jpg"), "a.jpg");
    assert.equal(dirName("/pics/a.jpg"), "/pics");
  });

  test("无目录时 dirName 返回空串", () => {
    assert.equal(dirName("a.jpg"), "");
  });
});

describe("joinPath", () => {
  test("分隔符跟随目录风格", () => {
    assert.equal(joinPath("D:\\pics", "_compressed"), "D:\\pics\\_compressed");
    assert.equal(joinPath("/pics", "_compressed"), "/pics/_compressed");
  });

  test("目录已带分隔符时不重复", () => {
    assert.equal(joinPath("D:\\pics\\", "_compressed"), "D:\\pics\\_compressed");
  });

  test("空目录返回子项，不产生根相对路径", () => {
    assert.equal(joinPath("", "_compressed"), "_compressed");
  });
});
