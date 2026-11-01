/**
 * `utils/albumHealth.ts` 单元测试
 *
 * 这里钉死的是几种最容易写错、且错了会误导用户的语义：
 *   1. 空相册不是故障（否则新装用户一进体检页满屏红）；
 *   2. 根目录失联要盖过一切（覆盖率数字此时没有意义，不该叠加噪音）；
 *   3. "抽样缺失"的措辞必须带抽样，不许冒充全量；
 *   4. 差个位数张不算不完整（阈值 INCOMPLETE_RATIO），避免狼来了。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  albumCoverage,
  albumIssues,
  albumLevel,
  healthSummary,
  sortBySeverity,
  type HealthIssue,
} from "./albumHealth.ts";
import type { AlbumHealthRow } from "../types/health.ts";

function row(over: Partial<AlbumHealthRow> = {}): AlbumHealthRow {
  return {
    album_id: 1,
    name: "川西合集",
    path: "D:\\photos\\川西合集",
    kind: "normal",
    photo_count: 100,
    size_bytes: 1024,
    scanned: 100,
    thumbs: 100,
    embeddings: 100,
    thumb_bytes: 100,
    root_exists: true,
    sampled: 50,
    missing_files: 0,
    ...over,
  };
}

const keys = (issues: HealthIssue[]) => issues.map((i) => i.key);

test("健康相册：没有任何问题", () => {
  assert.deepEqual(albumIssues(row()), []);
  assert.equal(albumLevel(row()), "ok");
});

test("空相册不是故障（新装用户不该一进来满屏红）", () => {
  const empty = row({ photo_count: 0, scanned: 0, thumbs: 0, embeddings: 0 });
  assert.deepEqual(albumIssues(empty), []);
  assert.equal(albumLevel(empty), "ok");
});

test("整册未入库 → 严重，且不给覆盖率噪音", () => {
  const r = row({ scanned: 0, thumbs: 0, embeddings: 0 });
  assert.equal(albumLevel(r), "bad");
  assert.ok(keys(albumIssues(r)).includes("not-scanned"));
  assert.ok(!keys(albumIssues(r)).includes("thumbs"), "整册未入库时不再叠加缺缩略图");
  assert.deepEqual(albumCoverage(r), { scan: 0, thumb: 0, embed: 0 });
});

test("差个位数张不算不完整（避免狼来了）", () => {
  const r = row({ photo_count: 100, scanned: 97, thumbs: 97, embeddings: 97 });
  assert.equal(albumLevel(r), "ok", "9 成以上入库就视为最新");
  assert.ok(!keys(albumIssues(r)).includes("partial-scan"));
});

test("入库缺口大 → 提示，并写清差多少张", () => {
  const r = row({ photo_count: 100, scanned: 40, thumbs: 40, embeddings: 40 });
  const issue = albumIssues(r).find((i) => i.key === "partial-scan");
  assert.ok(issue, "应当报入库不完整");
  assert.equal(issue.level, "warn");
  assert.match(issue.label, /差 60 张/);
});

test("根目录失联盖过一切：只报这一条", () => {
  const r = row({ root_exists: false, scanned: 0, thumbs: 0, embeddings: 0, missing_files: 50 });
  const issues = albumIssues(r);
  assert.equal(issues.length, 1);
  assert.equal(issues[0].key, "root-missing");
  assert.equal(albumLevel(r), "bad");
});

test("抽样缺失必须写清抽样口径，不许冒充全量", () => {
  const r = row({ sampled: 50, missing_files: 3 });
  const issue = albumIssues(r).find((i) => i.key === "files-missing");
  assert.ok(issue);
  assert.match(issue.label, /抽样 50 条中 3 条/);
  assert.equal(albumLevel(r), "bad");
});

test("缺缩略图按「已入库」为分母，不是按磁盘张数", () => {
  const r = row({ photo_count: 100, scanned: 100, thumbs: 20, embeddings: 100 });
  const issue = albumIssues(r).find((i) => i.key === "thumbs");
  assert.ok(issue);
  assert.match(issue.label, /缺缩略图（80 张）/);
  assert.equal(albumCoverage(r).thumb, 0.2);
});

test("向量从 0 到部分：措辞不同（没有 / 不全是两种问题）", () => {
  const none = albumIssues(row({ embeddings: 0 })).find((i) => i.key === "no-embeddings");
  const partial = albumIssues(row({ embeddings: 30 })).find(
    (i) => i.key === "partial-embeddings",
  );
  assert.ok(none && partial);
  assert.match(partial.label, /30\/100/);
});

test("汇总：三档计数 + 关键统计 + 覆盖率", () => {
  const rows = [
    row({ album_id: 1 }),
    row({ album_id: 2, name: "B", scanned: 0, thumbs: 0, embeddings: 0 }),
    row({ album_id: 3, name: "C", photo_count: 0, scanned: 0, thumbs: 0, embeddings: 0 }),
    row({ album_id: 4, name: "D", root_exists: false, scanned: 0, thumbs: 0, embeddings: 0 }),
  ];
  const s = healthSummary(rows);
  assert.equal(s.ok, 2, "健康 + 空相册都算正常");
  assert.equal(s.bad, 2);
  assert.equal(s.unscannedAlbums, 1, "空相册不算未入库");
  assert.equal(s.missingRootAlbums, 1);
  assert.equal(s.totalPhotos, 300);
  assert.equal(s.totalScanned, 100);
  assert.equal(Math.round(s.scanCoverage * 100), 33);
});

test("排序：严重在前，同档按影响面降序", () => {
  const rows = [
    row({ album_id: 1, name: "ok" }),
    row({ album_id: 2, name: "小缺口", photo_count: 100, scanned: 20, thumbs: 20, embeddings: 20 }),
    row({ album_id: 3, name: "大缺口", photo_count: 1000, scanned: 10, thumbs: 10, embeddings: 10 }),
  ];
  assert.deepEqual(
    sortBySeverity(rows).map((r) => r.name),
    ["大缺口", "小缺口", "ok"],
  );
});
