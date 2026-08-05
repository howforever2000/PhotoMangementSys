/**
 * AOP 风格日志装饰器（面向切面编程）
 *
 * `trace` 是一个高阶函数装饰器，自动包装任意函数：
 * - 调用前记录：函数名 + 参数描述
 * - 调用后记录：函数名 + 耗时 + 结果（或错误）
 *
 * 所有自建组件函数用它包装，实现统一日志记录。
 *
 * 用法：
 * ```ts
 * const myFn = trace("myFn", (a: number, b: number) => a + b);
 * ```
 * 或包装 async 函数：
 * ```ts
 * const load = trace("loadData", async () => { ... });
 * ```
 */

type AnyFunc = (...args: any[]) => any;

/** 格式化参数为可读字符串（截断避免过长） */
function fmtArgs(args: unknown[]): string {
  if (args.length === 0) return "";
  try {
    const s = JSON.stringify(args);
    return s && s.length > 200 ? s.slice(0, 200) + "…" : s;
  } catch {
    return String(args);
  }
}

/** 格式化结果为可读字符串（截断） */
function fmtResult(v: unknown): string {
  if (v === undefined) return "void";
  if (v === null) return "null";
  try {
    const s = JSON.stringify(v);
    return s && s.length > 150 ? s.slice(0, 150) + "…" : s;
  } catch {
    return String(v);
  }
}

/** 当前时间戳 */
function nowTs(): string {
  return new Date().toISOString().replace("T", " ").slice(0, 23);
}

/**
 * 包装同步/异步函数，自动记录调用前后日志
 *
 * @param name 函数名
 * @param fn 被包装的函数
 * @param desc 可选的静态描述（追加到日志）
 */
export function trace<T extends AnyFunc>(name: string, fn: T, desc = ""): T {
  const wrapped = function (this: unknown, ...args: Parameters<T>): ReturnType<T> {
    const descStr = desc ? ` | ${desc}` : "";
    const ts = nowTs();
    console.log(`[AOP] ${ts} CALL  ${name}(${fmtArgs(args)})${descStr}`);

    const start = performance.now();
    const finish = (resultTag: string) => {
      const elapsed = (performance.now() - start).toFixed(1);
      console.log(`[AOP] ${ts} RET   ${name} | ${elapsed}ms | ${resultTag}`);
    };

    try {
      const result = fn.apply(this, args);
      if (result instanceof Promise) {
        return result.then(
          (v) => {
            finish(`OK: ${fmtResult(v)}`);
            return v;
          },
          (e) => {
            console.log(`[AOP] ${ts} ERR   ${name} | ${String(e)}`);
            throw e;
          },
        ) as ReturnType<T>;
      }
      finish(`OK: ${fmtResult(result)}`);
      return result;
    } catch (e) {
      console.log(`[AOP] ${ts} ERR   ${name} | ${String(e)}`);
      throw e;
    }
  };
  return wrapped as T;
}
