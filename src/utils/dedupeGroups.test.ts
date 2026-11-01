/**
 * `utils/dedupeGroups.ts` 单元测试
 *
 * 背景（BUG-2026-1101-003）：相册 A 包含相册 B，两个都入库后同一张照片
 * 在「重复扫描清理」里被当成两份重复。后端已按真实路径身份归并，前端这里
 * 再兜一层 —— 这组断言钉死三件事：
 *   ① 组内同路径只留一张；
 *   ② 只剩一张的组整组消失（一张不叫重复）；
 *   ③ 保留项被合并掉时，保留标记要回落到仍在列表里的那张（不能指向不存在的路径）。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { dedupeItemsByPath, reclaimableOf, sanitizeReport, shortDir } from "./dedupeGroups.ts";
import type { DedupeReport, DupGroup, DupItem } from "../types/dedupe.ts";

function item(path: string, size = 1000): DupItem {
  return {
    path,
    dir: path.slice(0, path.lastIndexOf("\\")),
    album_id: 1,
    file_name: path.split("\\").pop() ?? path,
    size_bytes: size,
    width: 4000,
    height: 3000,
    modified_secs: 0,
  };
}

function group(over: Partial<DupGroup> & { items: DupItem[] }): DupGroup {
  const keep = over.keep_path ?? over.items[0].path;
  return {
    key: over.key ?? `k:${keep}`,
    kind: over.kind ?? "exact",
    similarity: over.similarity ?? 1,
    keep_path: keep,
    reclaimable_bytes: reclaimableOf(over.items, keep),
    items: over.items,
  };
}

function report(groups: DupGroup[]): DedupeReport {
  return {
    groups,
    total_photos: 10,
    similar_candidates: 0,
    skipped_unencoded: 0,
    exact_groups: groups.filter((g) => g.kind === "exact").length,
    similar_groups: groups.filter((g) => g.kind === "similar").length,
    threshold: 0.98,
    embedding_model: null,
    similar_skipped_reason: null,
    elapsed_ms: 1,
    heic: { available: true, codec: null, reason: null },
    hashed_files: 0,
  };
}

test("组内同路径（大小写不同）只保留一张", () => {
  const items = [item("D:\\a\\x.jpg"), item("d:\\A\\X.JPG"), item("D:\\a\\y.jpg")];
  const got = dedupeItemsByPath(items);
  assert.equal(got.length, 2);
  assert.equal(got[0].path, "D:\\a\\x.jpg", "保留首次出现的那条");
});

test("清洗后不足两张的组整组消失，且计入同址合并数", () => {
  const r = report([
    group({ items: [item("D:\\a\\x.jpg"), item("D:\\a\\x.jpg")] }),
    group({ items: [item("D:\\a\\p.jpg"), item("D:\\a\\q.jpg")] }),
  ]);
  const s = sanitizeReport(r);
  assert.equal(s.groups.length, 1, "只剩一组真正的重复");
  assert.equal(s.exact_groups, 1);
  assert.equal(s.collapsed_same_file, 1, "被合并掉的同址条目要报数");
});

test("保留项被合并掉时，保留标记回落到仍在列表里的那张", () => {
  const a = item("D:\\a\\x.jpg", 500);
  const b = item("D:\\a\\y.jpg", 900);
  const g = group({ items: [a, a, b], keep_path: a.path });
  const s = sanitizeReport(report([g]));
  assert.equal(s.groups.length, 1);
  assert.equal(s.groups[0].items.length, 2);
  assert.ok(
    s.groups[0].items.some((it) => it.path === s.groups[0].keep_path),
    "keep_path 必须指向仍在列表里的照片",
  );
  assert.equal(s.groups[0].reclaimable_bytes, 900, "可释放体积按清洗后的列表重算");
});

test("可释放体积 = 除保留外全部", () => {
  const items = [item("D:\\a\\1.jpg", 10), item("D:\\a\\2.jpg", 20), item("D:\\a\\3.jpg", 30)];
  assert.equal(reclaimableOf(items, "D:\\a\\2.jpg"), 40);
});

test("地址只留末尾两级，空地址给出可读兜底", () => {
  assert.equal(shortDir("D:\\YUAN HAO\\Pictures\\img\\手机照片\\2026"), "…\\手机照片\\2026");
  assert.equal(shortDir("D:\\a"), "D:\\a");
  assert.equal(shortDir(""), "地址未知");
  assert.equal(shortDir("D:/a/b/c"), "…\\b\\c", "斜杠写法也要认得");
});
