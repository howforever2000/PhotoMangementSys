<script setup lang="ts">
/**
 * 作业步骤卡（FEAT-168 · 只读）
 *
 * 后端 `jobs.py` 的 steps[] 在这里第一次真正可见：显示「第 N/M 步 · 耗时」，
 * 以及 `meta.plans` 的计划摘要与 `meta.verifications` 的复核结论。
 * 这是「中间过程可反思」在界面上的落点，也是修掉「meta.plans 前端零引用」的一跳。
 *
 * 只做展示，不持有任何作业状态、不发起操作。
 */
import { computed } from "vue";
import type { AiJobStatus, RunStep } from "../../../types/ai";
import { opById } from "../../../utils/toolCatalog";

const props = defineProps<{ job: AiJobStatus | null }>();

const steps = computed<RunStep[]>(() => props.job?.steps ?? props.job?.meta?.steps ?? []);
const plans = computed(() => props.job?.meta?.plans ?? []);
const verifications = computed(() => props.job?.meta?.verifications ?? []);
const hasContent = computed(
  () => steps.value.length > 0 || plans.value.length > 0 || verifications.value.length > 0,
);

/* 复核结论只做汇总计数：逐图 verdict 串在 200 张时会变成一堵墙，
   明细由工作台的「出片质检」面板（FEAT-173）承担。 */
const passCount = computed(() => verifications.value.filter((v) => v.verdict === "pass").length);
const failCount = computed(() => verifications.value.filter((v) => v.verdict === "fail").length);
const unknownCount = computed(
  () => verifications.value.length - passCount.value - failCount.value,
);
const firstIssue = computed(() => verifications.value.flatMap((v) => v.issues ?? [])[0] ?? "");

function statusText(s: RunStep): string {
  switch (s.status) {
    case "running": return "进行中";
    case "done": return "完成";
    case "skipped": return "已跳过";
    case "failed": return "未成功";
    default: return s.status;
  }
}
function opLabel(op: string): string {
  return opById(op)?.label ?? op;
}
</script>

<template>
  <section v-if="job && hasContent" class="job-steps" aria-label="作业步骤">
    <ol v-if="steps.length" class="js-list">
      <li v-for="s in steps" :key="`${s.index}-${s.op}`" class="js-item" :class="`is-${s.status}`">
        <span class="js-dot" aria-hidden="true" />
        <span class="js-label">{{ s.label || opLabel(s.op) }}</span>
        <span class="js-state">{{ statusText(s) }}</span>
        <span v-if="s.ms" class="js-ms">{{ s.ms }}ms</span>
        <span v-if="s.issues?.length" class="js-issue" :title="s.issues.join('；')">
          {{ s.issues[0] }}
        </span>
      </li>
    </ol>

    <div v-if="plans.length" class="js-plan">
      <span class="js-plan-label">本次计划</span>
      <span class="js-plan-steps">
        {{ plans[0].steps.map((s) => opLabel(s.op)).join(" › ") }}
      </span>
    </div>

    <!-- 复核结论只给汇总：逐图 verdict 串在 200 张时会变成一堵墙，
         明细交给工作台的「出片质检」面板（FEAT-173）。 -->
    <p v-if="verifications.length" class="js-verify">
      复核：
      <span :class="{ 'js-fail': failCount > 0 }">
        {{ verifications.length }} 张中 {{ passCount }} 通过<template v-if="failCount">、{{ failCount }} 异常</template>
        <template v-if="unknownCount">、{{ unknownCount }} 待核</template>
      </span>
      <span v-if="firstIssue" class="js-dim">（{{ firstIssue }}）</span>
    </p>
  </section>
</template>

<style scoped>
.job-steps {
  margin: 12px 0 0;
  padding: 12px 14px;
  border-radius: var(--r-ui);
  border: 1px solid var(--c-rule);
  background: var(--hover, rgba(120, 120, 130, 0.05));
}
.js-list {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 14px;
  margin: 0;
  padding: 0;
  list-style: none;
}
.js-item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--fs-meta);
  color: var(--fg-2);
}
.js-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--fg-3, var(--fg-2));
  flex-shrink: 0;
}
.is-running .js-dot {
  background: var(--c-accent);
}
.is-done .js-dot {
  background: color-mix(in srgb, var(--c-accent) 70%, transparent);
}
.is-failed .js-dot,
.js-fail {
  color: var(--c-danger);
}
.is-failed .js-dot {
  background: var(--c-danger);
}
.js-label {
  color: var(--fg);
}
.js-state {
  color: var(--fg-2);
}
.js-ms {
  color: var(--fg-2);
  font-variant-numeric: tabular-nums;
}
.js-issue {
  max-width: 220px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--c-danger);
}
.js-plan {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-top: 10px;
  padding-top: 10px;
  border-top: 1px solid var(--c-rule);
  font-size: var(--fs-meta);
}
.js-plan-label {
  flex-shrink: 0;
  font-size: var(--fs-label);
  letter-spacing: 0.14em;
  color: var(--fg-2);
}
.js-plan-steps {
  color: var(--fg-2);
}
.js-verify {
  margin: 8px 0 0;
  font-size: var(--fs-meta);
  color: var(--fg-2);
}
.js-dim {
  color: var(--fg-2);
}
</style>
