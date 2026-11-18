/**
 * 样片抽样（FEAT-174 / FEAT-180）
 * ===================================================================
 * 只做一件事：从全量照片里挑出跑样片的若干张。纯计算，不发请求、不碰 store。
 *
 * 调参字段不在这里：单步微调的参数表已改为 `op_catalog.json` 驱动
 * （`utils/workbenchPlan.ts` 的 `paramViews` / `paramsOverride`），
 * 不再维护一份手写白名单——那份表会与后端 `TUNABLE_PARAMS` 悄悄漂移。
 */

/**
 * 从全量里抽 ≤count 张当样片。
 *
 * 为什么不取前 N 张：批量导入常按时间排序，前 5 张可能是同一场景连拍，
 * 调出来好看不代表整批好看。按导入顺序**等距**取（分层的最简单形态），
 * 一批 268 张取第 0/53/107/160/214 张，首中尾都有。
 *
 * 纪律：抽样在**前端**完成——后端 recipe 工作流本来就按 paths 逐张跑，
 * 传 5 张 paths 就是 5 张样片，不需要新接口。
 */
export function pickSample(paths: string[], count: number): string[] {
  const list = paths.filter(Boolean);
  if (list.length <= count) return [...list];
  const out: string[] = [];
  for (let i = 0; i < count; i++) {
    // 等距落点：首张固定，尾张尽量覆盖；round 保证不会重复取同一张
    const idx = Math.round((i * (list.length - 1)) / (count - 1));
    if (!out.includes(list[idx])) out.push(list[idx]);
  }
  return out;
}
