/**
 * 工作台 / 素材托盘的缩略图地址表（FEAT-180）
 * ===================================================================
 * 两处页面各写过一份一模一样的 `srcMap + ensureSrc`（对象展开写入、只增不减），
 * 这里收口成一个组合式：
 *   - **增量写入**：直接给响应式对象赋键，不再整对象展开（导入 N 张是 O(n²) 拷贝）；
 *   - **并发去重**：同一路径解析中不重复 invoke；
 *   - **按工作集剪枝**：移出工作集的路径通过 `prune(live)` 回收，条目数上界 =
 *     当前工作集（含产物路径），不再是「本会话导入过的所有照片」。
 *
 * 为什么不做 LRU 淘汰：缩略图地址一旦被淘汰，`ensureSrc` 没有「重新解析」的触发点
 * （只在路径集合变化时调用），仍在工作集里的图会直接变空白。按集合剪枝才安全。
 */
import { ref } from "vue";
import { resolvePhotoSrc } from "../utils/photoSrc";

export function usePhotoSrcMap() {
  /** 路径 → 可放进 `<img src>` 的地址（解析失败为空串，模板显示占位） */
  const srcMap = ref<Record<string, string>>({});
  const pending = new Set<string>();

  async function ensureSrc(path: string): Promise<void> {
    if (!path || srcMap.value[path] !== undefined || pending.has(path)) return;
    pending.add(path);
    try {
      srcMap.value[path] = await resolvePhotoSrc(path);
    } catch {
      // 解析失败留空 → 模板回落成占位块，不阻断整块 UI
      srcMap.value[path] = "";
    } finally {
      pending.delete(path);
    }
  }

  /** 只保留 `live` 里仍存在的路径；移出工作集的条目立即释放。 */
  function prune(live: Iterable<string>): void {
    const keep = new Set(live);
    for (const path of Object.keys(srcMap.value)) {
      if (!keep.has(path)) delete srcMap.value[path];
    }
  }

  return { srcMap, ensureSrc, prune };
}
