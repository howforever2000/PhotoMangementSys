/**
 * 缩略图请求队列：全局限流 + 分批复用 + 按 path 去重
 * ===================================================================
 *
 * # 为什么要有这一层（BUG-2026-1023-009 进程级崩溃）
 *
 * FEAT-1021 把缩略图渲染版本升到 v2（按 EXIF Orientation 转正）后，
 * `thumb_photo_hash`（表主键）与 `file_fingerprint`（磁盘文件名）**一起变**，
 * 于是全库 2.5 万张缩略图一次性全部失效 —— 升级后第一次打开时间线是
 * 「全冷缓存」。而时间线原来的实现是按 album 分组后 `Promise.all`：
 *
 *   实测（PMS_LOG_LEVEL=debug）：一次点开 = **129 路并发**
 *   `get_photo_thumbs`，单个相册最多 2876 条路径；
 *   后端每路再开 min(核数,8) 线程 → 进程线程数 732、私有内存 1.2GB，
 *   最终线程栈无法继续提交，Windows 报 0xC00000FD（栈溢出）整进程退出。
 *
 * v0.4.2 之前不炸，是因为缓存是热的（几乎全是 hit，现场生成的只有一两张）。
 *
 * # 本模块的职责边界
 *
 * 只做「带宽管理」，不改业务语义：
 * - **全局在飞上限** `MAX_INFLIGHT`：同时最多这么多个 invoke 在跑，
 *   请求再多也只是排队（后端还有进程级闸门兜底）；
 * - **单请求批量** `BATCH`：IPC 载荷可控，单批最坏耗时可估；
 * - **按 path 去重**：同一张照片被多个页面/多个相册同时要时只请求一次
 *   （顺带避免同一缩略图文件被并发写坏）；
 * - **结果缓存**：拿到的 path→thumb 留在模块级 Map 里，页面重挂载可先回填，
 *   不重复请求。
 *
 * 注意：调用方仍应只把**可见的**照片交进来（见 Timeline.vue 的
 * IntersectionObserver 惰性入队）。限流只能止血，别用它给「一次要全库」兜底。
 */

import { invoke } from "@tauri-apps/api/core";

/** 同时在飞的请求数上限（3 路足够喂满后端的 4 个生成名额，又不至于抢占） */
const MAX_INFLIGHT = 3;

/** 单次请求最多携带的路径数 */
const BATCH = 60;

interface Job {
  albumId: number;
  paths: string[];
}

/** 排队中的请求（先进先出） */
const queue: Job[] = [];

/** path → 已拿到的缩略图路径 */
const ready = new Map<string, string>();

/** path → 等待该结果的回调（排队中/在飞中才有） */
const waiting = new Map<string, ((thumb: string) => void)[]>();

/** 当前在飞请求数 */
let inflight = 0;

/** 已解析的缩略图（命中可直接渲染，无需再请求） */
export function cachedThumb(path: string): string | undefined {
  return ready.get(path);
}

/** 清掉单张缓存（照片被替换/删除后调用，避免渲染旧图） */
export function forgetThumb(path: string): void {
  ready.delete(path);
}

/** 给一批 path 的所有等待者放行（thumb 为空串表示本次没拿到） */
function settle(paths: string[], got: Map<string, string>): void {
  for (const path of paths) {
    const waiters = waiting.get(path);
    if (!waiters) continue;
    waiting.delete(path);
    const thumb = got.get(path) ?? "";
    for (const resolve of waiters) resolve(thumb);
  }
}

/** 队列泵：补满在飞名额 */
function pump(): void {
  while (inflight < MAX_INFLIGHT && queue.length > 0) {
    const job = queue.shift()!;
    inflight += 1;
    invoke<[string, string][]>("get_photo_thumbs", { albumId: job.albumId, paths: job.paths })
      .then((pairs) => {
        const got = new Map<string, string>();
        for (const [path, thumb] of pairs) {
          if (!path || !thumb) continue;
          ready.set(path, thumb);
          got.set(path, thumb);
        }
        settle(job.paths, got);
      })
      .catch(() => {
        // 整批失败：放行等待者（卡片回退占位），下次入队可重试
        settle(job.paths, new Map<string, string>());
      })
      .finally(() => {
        inflight -= 1;
        pump();
      });
  }
}

/**
 * 批量取缩略图（全局限流 + 去重 + 分批）
 *
 * @param albumId 照片所属相册（后端按 album 组织缓存文件名）
 * @param paths   原图绝对路径（调用方负责只传需要的、通常是可见的那些）
 * @returns       本调用关心的 `[原图路径, 缩略图路径]`；失败项不在结果里
 *
 * 语义与原来的直接 invoke 一致（拿不到就不返回该项），因此调用点可以原样替换。
 */
export function getThumbs(albumId: number, paths: string[]): Promise<[string, string][]> {
  if (!paths.length) return Promise.resolve([]);

  const result: [string, string][] = [];
  const need: string[] = [];
  const waits: Promise<void>[] = [];
  const seen = new Set<string>();

  for (const path of paths) {
    if (!path || seen.has(path)) continue;
    seen.add(path);

    const hit = ready.get(path);
    if (hit) {
      result.push([path, hit]);
      continue;
    }

    // 已经在排队/在飞（别的页面要过同一张）：只挂等待，不重复请求
    if (!waiting.has(path)) need.push(path);
    waits.push(
      new Promise<void>((resolve) => {
        const waiters = waiting.get(path) ?? [];
        waiters.push((thumb) => {
          if (thumb) result.push([path, thumb]);
          resolve();
        });
        waiting.set(path, waiters);
      }),
    );
  }

  // 按 BATCH 切块入队（同一相册顺序不变，便于后端顺序落盘）
  for (let i = 0; i < need.length; i += BATCH) {
    queue.push({ albumId, paths: need.slice(i, i + BATCH) });
  }
  if (need.length > 0) pump();

  if (waits.length === 0) return Promise.resolve(result);
  return Promise.all(waits).then(() => result);
}

/** 测试用：重置内部状态（生产代码不要调用） */
export function __resetThumbQueue(): void {
  queue.length = 0;
  ready.clear();
  waiting.clear();
  inflight = 0;
}
