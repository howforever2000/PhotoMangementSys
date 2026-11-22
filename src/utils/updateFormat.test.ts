/**
 * `utils/updateFormat.ts` 单元测试
 *
 * 这四个函数决定更新弹窗里「还要等多久 / 上次什么时候查的 / 新版本什么时候发的」，
 * 全都是「算错了但不会报错」的类型 —— 只会让用户误判进度，所以必须钉住边界。
 *
 * 时区处理：所有期望值都不硬编码 UTC 偏移。
 * 需要确定时刻的地方一律**用本地时间分量构造** `Date`
 * （`new Date(2026, 10, 22, 9, 7)`），这样无论 CI 在哪个时区跑，
 * 本地 getter 读回来的都是同一组数字。
 */
import { describe, test } from "node:test";
import assert from "node:assert/strict";

import { formatCheckedAt, formatEta, formatPublishedAt, formatSpeed } from "./updateFormat.ts";

describe("formatSpeed", () => {
  test("正常速度带 /s 后缀", () => {
    assert.equal(formatSpeed(1.2 * 1024 * 1024), "1.2 MB/s");
    assert.equal(formatSpeed(480 * 1024), "480 KB/s");
  });

  test("0 与负值不显示（未知 ≠ 0 KB/s）", () => {
    assert.equal(formatSpeed(0), "");
    assert.equal(formatSpeed(-1024), "");
    assert.equal(formatSpeed(Number.NaN), "");
  });
});

describe("formatEta", () => {
  test("不足一分钟按秒", () => {
    assert.equal(formatEta(0, 45 * 1000, 1000), "剩余 45 秒");
  });

  test("秒数向上取整（不足 1 秒也不会显示 0）", () => {
    // 剩 1001 字节 / 1000 Bps = 1.001 秒 → 2 秒
    assert.equal(formatEta(0, 1001, 1000), "剩余 2 秒");
  });

  test("分钟级用「分 秒」", () => {
    assert.equal(formatEta(10, 210, 1), "剩余 3 分 20 秒");
  });

  test("≥1 小时只用「小时 + 分」，分钟补零", () => {
    // 3900 秒 = 1 小时 05 分
    assert.equal(formatEta(0, 3900, 1), "剩余 1 小时 05 分");
  });

  test("分钟向上取整会进位到小时", () => {
    // 3599 秒 → 向上取整成 60 分 → 1 小时 00 分（不能显示「0 小时 60 分」）
    assert.equal(formatEta(0, 3599, 1), "剩余 1 小时 00 分");
  });

  test("速度/体积/已下满三种情况都返回空串", () => {
    assert.equal(formatEta(100, 1000, 0), "");
    assert.equal(formatEta(100, 0, 1000), "");
    assert.equal(formatEta(1000, 1000, 1000), "");
    assert.equal(formatEta(2000, 1000, 1000), "");
  });
});

describe("formatCheckedAt", () => {
  test("从未检查过给「未检查」", () => {
    assert.equal(formatCheckedAt(0), "未检查");
    assert.equal(formatCheckedAt(-1), "未检查");
  });

  test("本地时间 MM-DD HH:mm 且补零", () => {
    // 用本地分量构造：期望值在任何时区都成立
    const ms = new Date(2026, 2, 5, 9, 7, 3).getTime();
    assert.equal(formatCheckedAt(ms), "03-05 09:07");
    // 与 new Date(ms) 的手算结果一致（防止实现里混进 UTC getter）
    const d = new Date(ms);
    const want = `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")} ${String(
      d.getHours(),
    ).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
    assert.equal(formatCheckedAt(ms), want);
  });
});

describe("formatPublishedAt", () => {
  test("空串与非法值返回空串（整段不渲染）", () => {
    assert.equal(formatPublishedAt(""), "");
    assert.equal(formatPublishedAt("not-a-date"), "");
    assert.equal(formatPublishedAt("2026-13-45T99:99:99Z"), "");
  });

  test("按本地时区取 YYYY-MM-DD", () => {
    // 本地正午构造再转 ISO：跨时区解析回来仍是同一天
    const ms = new Date(2026, 10, 22, 12, 0, 0).getTime();
    assert.equal(formatPublishedAt(new Date(ms).toISOString()), "2026-11-22");
  });
});
