import test from "node:test";
import assert from "node:assert/strict";

import { thumbStatusText } from "./workbenchThumb.ts";

const item = (status: string, progress = 0) =>
  ({ status, progress }) as Parameters<typeof thumbStatusText>[0];

test("thumbStatusText：胶片条紧凑口径保持原样", () => {
  assert.equal(thumbStatusText(item("queued"), "compact"), "排队");
  assert.equal(thumbStatusText(item("running", 42), "compact"), "42%");
  assert.equal(thumbStatusText(item("done", 100), "compact"), "✓");
  assert.equal(thumbStatusText(item("failed"), "compact"), "!");
  assert.equal(thumbStatusText(item("idle"), "compact"), "");
});

test("thumbStatusText：网格完整口径保持原样", () => {
  assert.equal(thumbStatusText(item("queued"), "verbose"), "排队");
  assert.equal(thumbStatusText(item("running", 42), "verbose"), "处理中 42%");
  assert.equal(thumbStatusText(item("done", 100), "verbose"), "完成");
  assert.equal(thumbStatusText(item("failed"), "verbose"), "失败");
  assert.equal(thumbStatusText(item("idle"), "verbose"), "待处理");
});
