/**
 * useRememberedParams —— 把一组工具参数绑到「记忆」持久层（绑定层）
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 面板怎么用（每个工具有滑条的面板只加三行）
 * ──────────────────────────────────────────────────────────────────────────
 *   const { params, resetToDefault } = useRememberedParams("film_sim", {
 *     lut: "provia", strength: 100, look: 100, grade: "", gradeStrength: 100,
 *     grain: 0, vignette: 0,
 *   }, { clamp: { strength: [0, 100], look: [60, 140] } });
 *   const strength = toRef(params, "strength");
 *   // 模板里 v-model 不动：toRef 是可写 ref，和原来的 ref(...) 用法一致
 *
 * 为什么用 `toRef(params, key)` 而不是让面板把模板全改成 `params.xxx`：
 * 6 个面板的模板合计几十处绑定，改名只会制造无意义 diff；`toRef` 保持
 * 「一个值一个 ref」的原有写法，模板零改动。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么写盘要 debounce，且卸载时补落一次
 * ──────────────────────────────────────────────────────────────────────────
 * 拖滑条会连续触发几十次 `input`：每次都写 localStorage 会卡 UI（同步 IO）且
 * 把 JSON 反复序列化。300ms 防抖足够「停手即记住」。
 * 但用户可能「拖完立刻切走工具」—— 面板随即卸载，防抖计时器会被丢掉，
 * 所以 onBeforeUnmount 里必须 flush 一次，否则正好丢最后一次改动。
 *
 * 复位（恢复默认）要**跳过**随之而来的 watch 回调：否则刚清掉存储，300ms 后又
 * 把默认值写回去了，白白留一条「用户从没调过」的记录。
 *
 * 同理，写盘前先比一次「是否仍然全是默认值」：只进去看过、没动过任何滑条的工具
 * 不落记录 —— 存储里只该有用户真的调过的那几个工具，否则「恢复默认」看起来像没生效。
 */
import { onBeforeUnmount, reactive, watch } from "vue";
import {
  clearToolParams,
  differsFromDefaults,
  loadToolParams,
  saveToolParams,
  type RememberOptions,
} from "../utils/toolParamsMemory";

/** 停手多久后写盘 */
export const PARAMS_SAVE_DEBOUNCE_MS = 300;

export interface RememberedParams<T extends Record<string, unknown>> {
  /** 参数容器：字段值与面板模板一一对应（用 toRef 取单个字段即可） */
  params: T;
  /** 恢复本工具默认值，并清掉已存记录 */
  resetToDefault: () => void;
  /** 立刻把待写改动落盘（卸载时自动调用；手动 flush 仅供测试/极端时序） */
  flush: () => void;
}

export function useRememberedParams<T extends Record<string, unknown>>(
  toolId: string,
  defaults: T,
  opts: RememberOptions = {},
): RememberedParams<T> {
  const params = reactive({
    ...defaults,
    ...loadToolParams(toolId, defaults, opts),
  }) as T;

  let timer: ReturnType<typeof setTimeout> | null = null;
  /** 复位期间抑制一次自动保存（见文件头注释） */
  let suppressNextSave = false;

  function cancelTimer(): void {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  }

  function persist(): void {
    cancelTimer();
    const values = params as Record<string, unknown>;
    // 全是默认值 = 用户没调过：不留记录（否则存储里堆满「进来过就算记住」的空条目）
    if (differsFromDefaults(toolId, values, defaults)) {
      saveToolParams(toolId, values);
    } else {
      clearToolParams(toolId);
    }
  }

  watch(
    params,
    () => {
      if (suppressNextSave) {
        suppressNextSave = false;
        return;
      }
      cancelTimer();
      timer = setTimeout(() => {
        timer = null;
        persist();
      }, PARAMS_SAVE_DEBOUNCE_MS);
    },
    { deep: true },
  );

  // 面板被切换/卸载时把最后 300ms 内的改动补落，避免「拖完立刻切走」丢值
  onBeforeUnmount(persist);

  function resetToDefault(): void {
    cancelTimer();
    suppressNextSave = true;
    Object.assign(params, defaults);
    clearToolParams(toolId);
  }

  return { params, resetToDefault, flush: persist };
}
