/**
 * 工作台预览缓存的纯逻辑单测（与仓库其余 `src/utils/*.test.ts` 同用 node:test）
 */
import assert from "node:assert/strict";
import { test, describe } from "node:test";

import {
  PREVIEW_CACHE_LIMIT,
  getLru,
  previewCacheKey,
  putLru,
  stableKey,
} from "./previewCache.ts";

describe("stableKey", () => {
  test("键顺序不同但内容相同 → 同一个键（否则缓存永远不命中）", () => {
    assert.equal(stableKey({ a: 1, b: 2 }), stableKey({ b: 2, a: 1 }));
  });

  test("值不同 → 不同的键", () => {
    assert.notEqual(stableKey({ slim: 0.3 }), stableKey({ slim: 0.5 }));
  });

  test("嵌套对象稳定，数组顺序是语义的一部分", () => {
    assert.equal(
      stableKey({ steps: [{ op: "a" }, { op: "b" }] }),
      stableKey({ steps: [{ op: "a" }, { op: "b" }] }),
    );
    assert.notEqual(
      stableKey({ steps: [{ op: "a" }, { op: "b" }] }),
      stableKey({ steps: [{ op: "b" }, { op: "a" }] }),
    );
  });

  test("null / undefined / 基础类型", () => {
    assert.equal(stableKey(null), "");
    assert.equal(stableKey(undefined), "");
    assert.equal(stableKey(3), "3");
    assert.equal(stableKey(true), "true");
    assert.equal(stableKey(0), "0");
  });
});

describe("previewCacheKey", () => {
  test("同一图同一参数 → 同键", () => {
    const a = previewCacheKey({ path: "D:/a.jpg", recipeId: "natural", maxEdge: 1280 });
    const b = previewCacheKey({ path: "D:/a.jpg", recipeId: "natural", maxEdge: 1280 });
    assert.equal(a, b);
  });

  test("换图 / 换配方 / 换参数 / 换长边都会换键", () => {
    const base = previewCacheKey({ path: "D:/a.jpg", recipeId: "natural", maxEdge: 1280 });
    assert.notEqual(base, previewCacheKey({ path: "D:/b.jpg", recipeId: "natural", maxEdge: 1280 }));
    assert.notEqual(base, previewCacheKey({ path: "D:/a.jpg", recipeId: "vivid", maxEdge: 1280 }));
    assert.notEqual(base, previewCacheKey({ path: "D:/a.jpg", recipeId: "natural", maxEdge: 640 }));
    assert.notEqual(
      base,
      previewCacheKey({
        path: "D:/a.jpg",
        recipeId: "natural",
        maxEdge: 1280,
        paramsOverride: { local_film: { strength: 0.5 } },
      }),
    );
  });
});

describe("LRU 缓存", () => {
  test("命中会刷新顺序：刚用过的不会被淘汰", () => {
    const cache = new Map<string, string>();
    putLru(cache, "a", "A", 2);
    putLru(cache, "b", "B", 2);
    // 命中 a → a 变成最新，b 才是最久未用
    assert.equal(getLru(cache, "a"), "A");
    putLru(cache, "c", "C", 2);
    assert.equal(cache.has("a"), true);
    assert.equal(cache.has("b"), false);
    assert.equal(cache.has("c"), true);
  });

  test("未超限时不会丢条目", () => {
    const cache = new Map<string, string>();
    putLru(cache, "a", "A", 3);
    putLru(cache, "b", "B", 3);
    assert.equal(cache.size, 2);
  });

  test("重复写同一个键不会让 size 增长", () => {
    const cache = new Map<string, string>();
    putLru(cache, "a", "A");
    putLru(cache, "a", "A2");
    assert.equal(cache.size, 1);
    assert.equal(cache.get("a"), "A2");
  });

  test("默认上限是 8 条（内存预算：每条约 340~600KB）", () => {
    assert.equal(PREVIEW_CACHE_LIMIT, 8);
    const cache = new Map<string, string>();
    for (let i = 0; i < 12; i += 1) putLru(cache, `k${i}`, `v${i}`);
    assert.equal(cache.size, 8);
    assert.equal(cache.has("k3"), false);
    assert.equal(cache.has("k11"), true);
  });

  test("getLru 未命中返回 undefined 且不改变内容", () => {
    const cache = new Map<string, string>();
    putLru(cache, "a", "A");
    assert.equal(getLru(cache, "zzz"), undefined);
    assert.equal(cache.size, 1);
  });
});
