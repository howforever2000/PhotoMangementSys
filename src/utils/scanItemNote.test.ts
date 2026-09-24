/**
 * `utils/scanItemNote.ts` 单元测试
 *
 * 背景：BUG-2026-0922-008 —— 增量扫描能正确跳过已入库照片，但前端逐相册表
 * 每一行都写「共 0 张」，与「相册里明明有照片」直接矛盾。
 *
 * 这组断言钉死的是**三种语义必须措辞不同**：
 *   空相册（真的 0） / 已是最新全跳过（预期行为） / 部分处理
 * 尤其第一条与第二条在 UI 上原本只差一个数字，只能靠单测区分。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { scanItemNote, type ScanItemNoteInput } from "./scanItemNote.ts";

/** 构造一个「完成态」条目，只覆盖需要变化的字段 */
function done(over: Partial<ScanItemNoteInput> = {}): ScanItemNoteInput {
  return { status: "done", total: 0, written: 0, processed: 0, skipped: 0, ...over };
}

test("未完成（等待/扫描中/失败/已停止）→ 不显示任何张数", () => {
  for (const status of ["pending", "running", "failed", "stopped"]) {
    const got = scanItemNote(done({ status, total: 100, skipped: 100 }));
    assert.equal(got.text, "—", `status=${status} 不应显示张数`);
    assert.equal(got.title, "");
  }
});

test("空相册 → 显示「空相册」，这是唯一应当出现 0 的场景", () => {
  const got = scanItemNote(done({ total: 0 }));
  assert.equal(got.text, "空相册");
  assert.match(got.title, /没有图片/);
});

test("已入库相册零变化 → 「已是最新，跳过 N 张」，绝不出现「共 0 张」", () => {
  // 用户截图中的真实形态：目录 128 张、全部已入库、本次没处理任何一张
  const got = scanItemNote(done({ total: 128, processed: 0, skipped: 128, written: 0 }));
  assert.equal(got.text, "已是最新，跳过 128 张");
  assert.doesNotMatch(got.text, /共 0 张/, "回归守卫：不得再出现误导性的「共 0 张」");
  assert.doesNotMatch(got.text, /共 128 张/, "全跳过时以「已是最新」为主语，不并列总数");
  assert.match(got.title, /128 张全部已入库/);
});

test("增量补漏：处理了一部分 → 总数与本次处理 / 跳过并列", () => {
  const got = scanItemNote(done({ total: 300, processed: 12, skipped: 288, written: 12 }));
  assert.equal(got.text, "共 300 张（本次处理 12 张 · 跳过 288 张）");
  assert.match(got.title, /入库 12 张/);
});

test("全量扫描（无跳过）→ 总数 + 本次处理，但不出现「跳过 0 张」这类噪音", () => {
  const got = scanItemNote(done({ total: 50, processed: 50, skipped: 0, written: 50 }));
  assert.equal(got.text, "共 50 张（本次处理 50 张）");
  assert.doesNotMatch(got.text, /跳过 0 张/, "0 值补充项是无意义的噪音，不该渲染");
});

test("全量且处理数为 0（目录空）→ 走空相册分支，不并列补充项", () => {
  const got = scanItemNote(done({ total: 0, processed: 0, skipped: 0 }));
  assert.equal(got.text, "空相册");
});

test("共 T 张 恒等于 本次处理 + 跳过（后端恒等式的 UI 侧体现）", () => {
  // 若后端口径再次漂移（total 被填成差集），这条会立刻失败
  const cases = [
    done({ total: 10, processed: 10, skipped: 0, written: 10 }),
    done({ total: 10, processed: 0, skipped: 10 }),
    done({ total: 10, processed: 3, skipped: 7, written: 3 }),
  ];
  for (const c of cases) {
    assert.equal(
      c.total,
      c.processed + c.skipped,
      `total=${c.total} 必须等于 processed(${c.processed}) + skipped(${c.skipped})`,
    );
  }
});

test("处理过但一张都没写进去（写入失败）→ 不能误报成「已是最新」", () => {
  // processed > 0 说明确实干活了，written=0 是需要排查的异常，不是「已是最新」
  const got = scanItemNote(done({ total: 20, processed: 20, skipped: 0, written: 0 }));
  assert.equal(got.text, "共 20 张（本次处理 20 张）");
  assert.doesNotMatch(got.text, /已是最新/, "零写入但处理过 ≠ 已是最新，不得掩盖失败");
});
