/**
 * 工作台「出片质检」摘要（FEAT-173）
 * ===================================================================
 * 只做一件事：把后端 `job.meta.verifications[]`（每张图一条 critic 复核）
 * 汇总成界面能直接渲染的几行读数，不做任何判定逻辑、不发请求。
 *
 * 为什么单独成文件：这个汇总要同时服务右栏「质检」面板与中栏「样片条」的
 * 角标，逻辑放组件里会被复制两份；且它是纯函数，可单测。
 *
 * 数据形状（后端 `critic.compare` 输出，见 python/ai_studio/critic.py）：
 *   { verdict: "pass"|"fail"|"unknown", score: number, dims: {...}, issues: string[] }
 */
import type { Verification } from "../types/ai";

export type VerdictKind = Verification["verdict"];

export interface QcSummary {
  /** 有复核结果的张数 */
  total: number;
  pass: number;
  fail: number;
  /** unknown 不冒充通过：指标无法证明退化时就是 unknown（见 critic.py 注释） */
  unknown: number;
  /** 汇总后的去重问题清单 */
  issues: string[];
}

/** 空摘要（没有作业时用，避免模板里到处判空） */
export function emptyQc(): QcSummary {
  return { total: 0, pass: 0, fail: 0, unknown: 0, issues: [] };
}

/**
 * 汇总复核结果。
 * `issues` 去重但保序：同一类问题在整批里只显示一次，避免 200 张图刷出 200 行。
 */
export function summarizeQc(list: Verification[] | undefined | null): QcSummary {
  if (!list || !list.length) return emptyQc();
  const out = emptyQc();
  out.total = list.length;
  const seen = new Set<string>();
  for (const v of list) {
    const verdict = v?.verdict;
    if (verdict === "pass") out.pass += 1;
    else if (verdict === "fail") out.fail += 1;
    else out.unknown += 1;
    for (const issue of v?.issues ?? []) {
      const s = String(issue).trim();
      if (s && !seen.has(s)) {
        seen.add(s);
        out.issues.push(s);
      }
    }
  }
  return out;
}

/** 单条复核的角标文字（样片条与图块共用） */
export function verdictLabel(v: Verification | undefined | null): string {
  if (!v) return "";
  if (v.verdict === "pass") return "通过";
  if (v.verdict === "fail") return "异常";
  return "待核";
}

/**
 * 整批结论：只要有一张 fail 就是「有异常」；全 pass 才是「通过」；
 * 其余（含全 unknown）一律「待核」—— 绝不把 unknown 显示成通过。
 */
export function qcHeadline(s: QcSummary): string {
  if (!s.total) return "尚未出片";
  if (s.fail > 0) return `${s.fail} 张指标异常`;
  if (s.pass === s.total) return "全部通过";
  return "部分待核";
}

/** 质检行的颜色语义（模板里用 class 而非内联色值） */
export function qcTone(s: QcSummary): "idle" | "ok" | "warn" | "muted" {
  if (!s.total) return "idle";
  if (s.fail > 0) return "warn";
  if (s.pass === s.total) return "ok";
  return "muted";
}
