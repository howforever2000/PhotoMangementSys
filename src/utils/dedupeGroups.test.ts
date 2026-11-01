/**
 * `utils/dedupeGroups.ts` 单元测试
 *
 * 背景（BUG-2026-1101-003 / 005）：
 *   ① 相册 A 包含相册 B，两个都入库后同一张照片被当成两份重复（后端真实路径身份解决）；
 *   ② 更常见的情形是**同一张照片在同一个文件夹里有多份**（内容相同、文件名不同），
 *      后端按文件给 6 条，界面上就是 6 张一样的缩略图，且其中几条与「保留」那张地址相同，
 *      用户会以为重复列出、更怕误删想留的那张。
 *
 * 这组断言钉死折叠与删除边界：
 *   ① 一组的卡片数 = 不同文件夹数（保留文件夹排第一）；
 *   ② 可删路径永不含保留那张，也永不含保留文件夹里的其它副本；
 *   ③ 折叠后没有可删副本的组整组消失，可释放体积按可删路径重算。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  cellBytes,
  dedupeItemsByPath,
  deletablePathsOf,
  folderCells,
  reclaimableOf,
  sanitizeReport,
} from "./dedupeGroups.ts";
import type { DedupeReport, DupGroup, DupItem } from "../types/dedupe.ts";

function item(path: string, dir: string, size = 1000): DupItem {
  return {
    path,
    dir,
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
    reclaimable_bytes: over.reclaimable_bytes ?? 0,
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

const PIC = "D:\\P\\img\\Pictures";
const PHONE = "D:\\P\\img\\手机照片\\无地点";

/** 还原用户现场：同一张照片在 Pictures 有 3 份、在 手机照片\无地点 也有 3 份 */
function userScene(): DupGroup {
  const items = [
    item(`${PIC}\\mmexport…866.jpg`, PIC, 14_158_816),
    item(`${PHONE}\\mmexport…866.jpg`, PHONE, 14_158_816),
    item(`${PIC}\\mmexport…592.jpg`, PIC, 14_158_816),
    item(`${PHONE}\\mmexport…592.jpg`, PHONE, 14_158_816),
    item(`${PIC}\\mmexport…957.jpg`, PIC, 14_158_816),
    item(`${PHONE}\\mmexport…957.jpg`, PHONE, 14_158_816),
  ];
  return group({ items, keep_path: items[0].path });
}

test("同一张照片散布在两个文件夹 → 折叠成两张卡，保留文件夹排第一", () => {
  const g = userScene();
  const cells = folderCells(g, g.keep_path);
  assert.equal(cells.length, 2, "6 个文件 → 2 张卡（一个文件夹一张）");
  assert.equal(cells[0].isKeepFolder, true);
  assert.equal(cells[0].items.length, 3, "保留文件夹里是 3 份");
  assert.equal(cells[1].dir, PHONE);
  assert.equal(cellBytes(cells[1]), 14_158_816 * 3, "卡片体积 = 该文件夹内全部副本之和");
});

test("可删路径只来自非保留文件夹：勾了也删不到想留的那张", () => {
  const g = userScene();
  const del = deletablePathsOf(g, g.keep_path);
  assert.equal(del.length, 3, "只会删另一个文件夹的 3 份");
  assert.ok(!del.includes(g.keep_path), "保留那张永不可删");
  assert.ok(
    del.every((p) => p.startsWith(PHONE)),
    "保留文件夹（Pictures）里的其它 2 份也不参与删除",
  );
  assert.equal(reclaimableOf(g, g.keep_path), 14_158_816 * 3);
});

test("组内同路径（大小写不同）只保留一张", () => {
  const items = [item("D:\\a\\x.jpg", "D:\\a"), item("d:\\A\\X.JPG", "d:\\A"), item("D:\\a\\y.jpg", "D:\\a")];
  const got = dedupeItemsByPath(items);
  assert.equal(got.length, 2);
  assert.equal(got[0].path, "D:\\a\\x.jpg", "保留首次出现的那条");
});

test("折叠后没有可删副本的组整组消失（同一文件夹里的重复不算待清理）", () => {
  const a = item("D:\\a\\1.jpg", "D:\\a");
  const b = item("D:\\a\\2.jpg", "D:\\a");
  const dirty = group({ items: [a, b] });
  assert.equal(deletablePathsOf(dirty, a.path).length, 0);
  const s = sanitizeReport(report([dirty]));
  assert.equal(s.groups.length, 0, "同一文件夹内的副本不构成「可清理」的组");
});

test("保留项被去重掉时回落到仍在列表里的那张，体积跟着重算", () => {
  const a = item("D:\\a\\x.jpg", "D:\\a", 500);
  const c = item("D:\\b\\x.jpg", "D:\\b", 900);
  const g = group({ items: [a, a, c], keep_path: a.path });
  const s = sanitizeReport(report([g]));
  assert.equal(s.groups.length, 1);
  assert.equal(s.groups[0].items.length, 2);
  assert.equal(s.collapsed_same_file, 1, "被合并掉的同址条目要报数");
  assert.equal(s.groups[0].reclaimable_bytes, 900, "可释放体积 = 另一个文件夹那份");
});
