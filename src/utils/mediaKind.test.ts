/**
 * `utils/mediaKind.ts` 单元测试
 *
 * 这份判定与后端 `src-tauri/src/media_kind.rs` 必须逐字一致：
 * 两边规则一旦漂移，表现是「相册里某类文件忽隐忽现」——
 * 后端列出来了、前端分栏却归不进任何一类，或反之。
 * 样例集与 Rust 侧 `media_kind::tests` 保持同批。
 */
import { describe, test } from "node:test";
import assert from "node:assert/strict";

import {
  extOf,
  filterByMedia,
  isAnimated,
  isStillImage,
  isVideo,
  mediaKindOf,
} from "./mediaKind.ts";

describe("extOf", () => {
  test("正反斜杠都认", () => {
    assert.equal(extOf("D:\\album\\a.JPG"), "jpg");
    assert.equal(extOf("/home/u/a.PNG"), "png");
  });

  test("目录里带点不影响，只看最后一段", () => {
    assert.equal(extOf("D:/v1.2/photo.jpg"), "jpg");
    assert.equal(extOf("D:/mp4/photo.jpg"), "jpg");
  });

  test("无扩展名 / 隐藏文件返回空串", () => {
    assert.equal(extOf("D:/album/README"), "");
    assert.equal(extOf("D:/album/.gitkeep"), "");
  });
});

describe("mediaKindOf", () => {
  test("静态图片", () => {
    for (const p of ["a.jpg", "a.JPEG", "a.png", "a.heic", "a.heif", "a.bmp", "a.tif"]) {
      assert.equal(mediaKindOf(p), "image", p);
      assert.ok(isStillImage(p), p);
    }
  });

  test("动图", () => {
    for (const p of ["a.gif", "a.GIF", "a.webp"]) {
      assert.equal(mediaKindOf(p), "animated", p);
      assert.ok(isAnimated(p), p);
      assert.ok(!isStillImage(p), p);
    }
  });

  test("视频", () => {
    for (const p of ["a.mp4", "a.MOV", "a.m4v", "a.wmv", "a.webm"]) {
      assert.equal(mediaKindOf(p), "video", p);
      assert.ok(isVideo(p), p);
      assert.ok(!isStillImage(p), p);
    }
  });

  test("故意不收的容器一律归 null（与后端 exotic_containers_rejected 同口径）", () => {
    for (const p of ["a.mkv", "a.avi", "a.3gp"]) {
      assert.equal(mediaKindOf(p), null, p);
    }
  });

  test("非媒体返回 null", () => {
    for (const p of ["a.txt", "a.pdf", "a", ""]) {
      assert.equal(mediaKindOf(p), null, p);
    }
    assert.equal(mediaKindOf(null), null);
    assert.equal(mediaKindOf(undefined), null);
  });
});

describe("filterByMedia", () => {
  const items = [
    { path: "D:/a/1.jpg" },
    { path: "D:/a/2.gif" },
    { path: "D:/a/3.mp4" },
    { path: "D:/a/4.png" },
  ];

  test("all 原样返回", () => {
    assert.equal(filterByMedia(items, "all").length, 4);
  });

  test("按类型过滤", () => {
    assert.deepEqual(
      filterByMedia(items, "image").map((x) => x.path),
      ["D:/a/1.jpg", "D:/a/4.png"],
    );
    assert.deepEqual(
      filterByMedia(items, "animated").map((x) => x.path),
      ["D:/a/2.gif"],
    );
    assert.deepEqual(
      filterByMedia(items, "video").map((x) => x.path),
      ["D:/a/3.mp4"],
    );
  });

  test("过滤不会就地改原数组", () => {
    filterByMedia(items, "video");
    assert.equal(items.length, 4);
  });
});
