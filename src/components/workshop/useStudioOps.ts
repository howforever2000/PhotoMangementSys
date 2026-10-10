import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

/**
 * Python 工坊微服务（算子）的访问封装
 * ====================================================================
 * 原先是 `WorkshopImageProcess.vue`（已随融合删除）内部的一坨逻辑，
 * 融合时整段搬到这里，让调色与算子共用一份服务访问。
 *
 * 与原组件的差异只有一处，但很关键：**输入不再取原图，而是取调用方给的
 * 预览字节**。融合前算子作用在原图上，与调色面板各看各的；融合后二者
 * 共享同一个编辑器，算子必须作用在「Rust 已渲染的结果」上，否则预览里
 * 看得到算子效果、导出却没有（调色会把算子效果盖掉）。
 *
 * ## 为什么还留着 Python
 *
 * 算子是「算子形态」的滤镜：参数少、不追求逐帧实时、而 OpenCV 的边界
 * 处理是几十年积累的。搬到 Rust 要重写并逐个对齐行为，收益只是省几百
 * 毫秒。基准调色恰好相反 —— 拖滑块要每帧出图，HTTP 往返根本没法用。
 */

/** 算子参数定义（来自后端 `GET /api/ops` 的 schema）。 */
export interface OpParam {
  name: string;
  label: string;
  type: "number" | "bool" | "enum";
  min?: number;
  max?: number;
  step?: number;
  default: number | string | boolean;
  choices?: { value: string; label: string }[];
}

/** 算子定义。 */
export interface OpDef {
  id: string;
  label: string;
  supportsMask: boolean;
  params: OpParam[];
}

/** 拉取算子列表的超时上限。 */
const OPS_TIMEOUT_MS = 12_000;

export function useStudioOps() {
  const ops = ref<OpDef[]>([]);
  const loading = ref(false);
  const error = ref("");
  /** 微服务基址（studio_ensure 成功后缓存） */
  let base = "";

  const ready = computed(() => ops.value.length > 0);

  async function ensureBase(): Promise<string> {
    if (!base) base = await invoke<string>("studio_ensure");
    return base;
  }

  /** 给 promise 套超时：后端命令无法取消，卡住时也必须回到失败态。 */
  function withTimeout<T>(p: Promise<T>, ms: number, msg: string): Promise<T> {
    let timer = 0;
    const timeout = new Promise<never>((_, reject) => {
      timer = window.setTimeout(() => reject(new Error(msg)), ms);
    });
    return Promise.race([p.finally(() => window.clearTimeout(timer)), timeout]);
  }

  /** 把异常翻译成用户能看懂的话。 */
  function describe(e: unknown): string {
    if (e instanceof DOMException && e.name === "AbortError") {
      return `本机处理服务 ${OPS_TIMEOUT_MS / 1000}s 内无响应`;
    }
    const m = String((e as Error)?.message ?? e);
    if (m.includes("超时")) return `本机处理服务 ${OPS_TIMEOUT_MS / 1000}s 内未就绪`;
    if (/fetch|network|load failed|ECONNREFUSED|Failed/i.test(m)) return "无法连接本机处理服务";
    return m;
  }

  /** 拉取算子注册表（schema 驱动参数表单）。 */
  async function loadOps(force = false): Promise<void> {
    if (ready.value && !force) return;
    loading.value = true;
    error.value = "";
    const ac = new AbortController();
    const abortTimer = window.setTimeout(() => ac.abort(), OPS_TIMEOUT_MS);
    try {
      const svc = await withTimeout(ensureBase(), OPS_TIMEOUT_MS, "服务启动超时");
      const resp = await fetch(`${svc}/api/ops`, { signal: ac.signal });
      if (!resp.ok) throw new Error(`服务返回 ${resp.status}`);
      const list = ((await resp.json()) as { ops?: OpDef[] }).ops ?? [];
      if (!list.length) throw new Error("服务未返回任何算子");
      ops.value = list;
    } catch (e) {
      // 失败即丢弃缓存基址：重试时重新 ensure
      base = "";
      error.value = describe(e);
    } finally {
      window.clearTimeout(abortTimer);
      loading.value = false;
    }
  }

  /**
   * 执行算子。
   *
   * @param image 输入图像字节（**调用方的预览**，不是原图）
   * @param mask  灰度蒙版 PNG（白=选中）。不支持蒙版的算子传 null。
   */
  async function apply(
    opId: string,
    values: Record<string, number | string | boolean>,
    image: Blob,
    mask: Blob | null,
  ): Promise<Blob> {
    const svc = await ensureBase();
    const form = new FormData();
    form.append("op", opId);
    form.append("params", JSON.stringify(values));
    form.append("image", image, "input.jpg");
    if (mask) form.append("mask", mask, "mask.png");

    const resp = await fetch(`${svc}/api/apply`, { method: "POST", body: form });
    if (!resp.ok) {
      const detail = await resp.text().catch(() => "");
      throw new Error(`服务返回 ${resp.status}${detail ? `: ${detail.slice(0, 200)}` : ""}`);
    }
    return await resp.blob();
  }

  return { ops, loading, error, ready, loadOps, apply, ensureBase };
}

/** 面板用的薄封装（保持与 props 形状一致）。 */
export type StudioOpsApi = ReturnType<typeof useStudioOps>;