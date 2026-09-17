/**
 * 给任意 Promise 加超时保护（BUG-2026-0921-003 面板「一直转圈」防线）。
 *
 * 场景：后端 invoke 在某些状态下可能长时间不返回（如识别服务就绪等待、模型
 * 加载），而 UI 的 busy 标记/下拉禁用只能靠 promise settle 复位 —— 一旦超时
 * 迟迟不来，界面就永久停在「检测中…」。这里统一兜底：超时即 reject，调用方在
 * finally 里复位状态并给出可重试的失败提示。
 *
 * 注意：超时只解除 UI 阻塞，不会取消后端任务（invoke 无取消语义），
 * 因此超时时间应大于该命令的正常耗时上限。
 */
export function withTimeout<T>(p: Promise<T>, ms: number, label: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error(`${label} 超时（${(ms / 1000).toFixed(0)}s 未响应）`));
    }, ms);
    p.then(
      (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      (e) => {
        clearTimeout(timer);
        reject(e);
      },
    );
  });
}

/** 性能设置面板各命令的超时上限（ms）：读配置类命令偏短，测速类偏长。 */
export const PERF_TIMEOUT = {
  /** /gpu /models /threads 读配置（含冷启动拉起服务 ≤25s） */
  read: 45_000,
  /** 设置类（服务端立即返回、后台重建） */
  write: 45_000,
  /** 单通道测速（服务端 60s 长超时） */
  benchmark: 120_000,
  /** 线程扫档（服务端 300s 长超时，一次测多档） */
  sweep: 360_000,
} as const;
