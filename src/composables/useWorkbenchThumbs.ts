/**
 * 工作台缩略图地址表（胶片条 / 网格专用）
 * ===================================================================
 * 为什么不能继续复用 `usePhotoSrcMap`（它返回**原图** URL）：
 *
 * `resolvePhotoSrc` 对非 HEIC 直接 `convertFileSrc(原图)` —— 也就是说胶片条里
 * 一个 96×64 的格子、网格里一个 148px 的格子，都在让 WebView2 解码一张
 * 24MP 的原图（解码后 RGBA 约 96MB）。一屏 10~18 格 = 10~18 次 24MP 解码，
 * 且滚动时会不断触发。
 *
 * 这里改成走后端 `workbench_thumb`（≤384px 的缓存 JPEG）：
 *   - 解码量降两个数量级（384px ≈ 0.44MP vs 24MP）；
 *   - 后端按 `(路径, 大小, mtime, 长边)` 落盘缓存，重复打开零开销；
 *   - 生成受缩略图那道全局闸约束，不会与杂志预览抢内存。
 *
 * 三处纪律：
 *   1. **并发上限**：导入 200 张时不能一次性发 200 个 invoke（后端虽有名额闸，
 *      但 IPC 与前端内存都会被撑）。这里限 3 路在飞，照抄 `thumbQueue` 的口径。
 *   2. **按工作集剪枝**：条目数上界 = 当前工作集，不随本会话导入过的张数增长。
 *   3. **失败留空**：解析失败保留空串，模板回落占位块，不阻断整块 UI。
 */
import { ref } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { trace } from "../utils/trace";

/** 工作台缩略图长边（与后端 `WORKBENCH_THUMB_EDGE` 一致） */
export const WB_THUMB_EDGE = 384;

/** 同时在飞的解析数上限（与 `utils/thumbQueue` 的 MAX_INFLIGHT 同口径） */
const MAX_INFLIGHT = 3;

/** AOP 包装：只包 invoke 本身，缓存命中不产生日志 */
const fetchThumb = trace(
  "工坊.缩略图",
  (path: string): Promise<string> =>
    invoke<string>("workbench_thumb", { path, maxEdge: WB_THUMB_EDGE }),
);

export function useWorkbenchThumbs() {
  const srcMap = ref<Record<string, string>>({});

  const pending = new Set<string>();
  const queue: string[] = [];
  let inflight = 0;

  function pump(): void {
    while (inflight < MAX_INFLIGHT && queue.length > 0) {
      const path = queue.shift()!;
      inflight += 1;
      fetchThumb(path)
        .then((p) => {
          // 与全仓其他位置一致：本地路径统一交给 convertFileSrc，
          // 不手写 asset:// 协议（不同平台/网关的 URL 形态不同）。
          srcMap.value[path] = p ? convertFileSrc(p) : "";
        })
        .catch(() => {
          // 解析失败留空 → 模板回落占位块
          srcMap.value[path] = "";
        })
        .finally(() => {
          pending.delete(path);
          inflight -= 1;
          pump();
        });
    }
  }

  /** 请求某张图的缩略图地址（已在排队/已有结果时直接返回） */
  function ensure(path: string): void {
    if (!path) return;
    if (srcMap.value[path] !== undefined || pending.has(path)) return;
    pending.add(path);
    queue.push(path);
    pump();
  }

  /** 只保留 `live` 里仍存在的路径；移出工作集的条目立即释放 */
  function prune(live: Iterable<string>): void {
    const keep = new Set(live);
    for (const path of Object.keys(srcMap.value)) {
      if (!keep.has(path)) delete srcMap.value[path];
    }
    // 队列里待处理的也一并清掉，避免为已经移出的图白跑后端
    for (let i = queue.length - 1; i >= 0; i -= 1) {
      if (!keep.has(queue[i])) {
        pending.delete(queue[i]);
        queue.splice(i, 1);
      }
    }
  }

  return { srcMap, ensure, prune };
}
