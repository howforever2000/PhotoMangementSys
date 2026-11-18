/**
 * 单步工具通用作业骨架（FEAT-168）
 *
 * 原来 11 个工具面板各自复制 5 段：输出目录默推、`jobOfTool+busy+canRun`、
 * `run()` 组 AiRunRequest、空目录 warning、`onCancel→activeJobId→cancel`，
 * 末尾还都挂一个 `<AiResultPanel>`。这里一次收敛，面板只负责自己的参数区。
 *
 * 编排放在本 composable（明确入口层），面板组件不承载作业状态机。
 */
import { computed, ref, watch, type Ref } from "vue";
import { useAiStudioStore } from "../stores/aiStudio";
import { useNotify } from "./useNotify";
import { dirName, joinPath } from "../utils/pathParts";
import { trace } from "../utils/trace";
import { toolLabel } from "../utils/toolCatalog";

export interface UseToolJobOptions {
  /** 工具 id（与 op_catalog.json 一致，也是 Python 作业名） */
  tool: string;
  paths: Ref<string[]>;
  /** 提交时附带的参数（由 SchemaForm 或面板自己的控件提供） */
  options: () => Record<string, unknown>;
  /** 运行按钮文案前缀，如「开始形变」 */
  runLabel?: string;
}

export function useToolJob(opts: UseToolJobOptions) {
  const store = useAiStudioStore();
  const notify = useNotify();

  const outputDir = ref("");
  const outputTouched = ref(false);

  // 输出目录默认跟随第一张素材的父目录 /_ai（沿用既有约定）
  watch(
    opts.paths,
    (paths) => {
      if (outputTouched.value) return;
      const first = paths[0];
      const dir = first ? dirName(first) : "";
      outputDir.value = dir ? joinPath(dir, "_ai") : "";
    },
    { immediate: true },
  );

  const job = computed(() => store.jobOfTool(opts.tool));
  const busy = computed(
    () => !!job.value && (job.value.status === "queued" || job.value.status === "running"),
  );
  const canRun = computed(() => opts.paths.value.length > 0 && !busy.value);

  /** 单步工具提交：收敛入口，AOP 记录工具名（id + 中文名）+ 参数键 + 耗时。 */
  const run = trace(
    `工坊.单步工具.${opts.tool}`,
    async function submitToolJob(): Promise<void> {
      if (!outputDir.value) {
        notify.warning("还没有输出目录", "先选择结果要放在哪个文件夹");
        return;
      }
      console.info(
        `[工坊][工具] ${opts.tool}（${toolLabel(opts.tool)}） n=${opts.paths.value.length} opts=[${Object.keys(opts.options()).join(",")}]`,
      );
      try {
        await store.run({
          tool: opts.tool,
          paths: opts.paths.value,
          prompt: "",
          outputDir: outputDir.value,
          options: opts.options(),
        });
      } catch (e) {
        notify.error("提交失败", String(e));
      }
    },
  );

  async function cancel(): Promise<void> {
    const id = store.activeJobId(opts.tool);
    if (id) await store.cancel(id);
  }

  function onDir(v: string): void {
    outputTouched.value = true;
    outputDir.value = v;
  }

  return { outputDir, outputTouched, job, busy, canRun, run, cancel, onDir };
}
