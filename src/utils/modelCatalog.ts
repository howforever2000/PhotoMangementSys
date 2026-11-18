/**
 * 模型下载项的「归属分区」（FEAT-165）
 *
 * Rust 的 `model_dl` 下载表是**一张表**：相册识别的语义 / 人脸 / OCR / 人物检测，
 * 以及创意工坊的本地增强模型（`local-esrgan` / `local-gfpgan`）都登记在里面，
 * 因此天然共用同一套下载器（多镜像、断点续传、进度、取消）。
 *
 * 但「共用下载器」不等于「共用入口」：扫描侧的 📦 模型管理只该关心识别链路，
 * 创意工坊的模型要回创意工坊下。两边若各自写一份过滤条件，很快就漂移
 * （一边忘了排除、另一边忘了包含）。所以归属判定集中在本文件 —— 纯函数、可测，
 * 视图层只拿结果渲染。
 *
 * 判定口径：注册名以 `local-` 开头 = 创意工坊（本地增强）模型；
 * 其余 = 扫描 / 识别链路模型。`openvino-runtime` 是运行时组件（FEAT-136），
 * 归「加速分配」分区单独管，两个模型列表都不该出现它 —— 于是在这里统一剔除，
 * 免得某个入口忘了过滤，把它算进「缺失 / 未就绪」。
 */

/** 两个产品分区：相册识别链路 / 创意工坊本地增强 */
export type ModelArea = "scan" | "workshop";

/** 创意工坊本地增强模型的注册名前缀（与 `model_dl.rs` 的 `local-*` 一致） */
export const WORKSHOP_MODEL_PREFIX = "local-";

/**
 * 运行时组件：不是模型，只有在「加速分配」分区里才谈得上开关与下载。
 * 两处入口各说一套状态是 FEAT-133 留下的教训。
 */
export const RUNTIME_COMPONENTS: ReadonlySet<string> = new Set(["openvino-runtime"]);

/** 某个下载项是不是创意工坊（本地增强）模型 */
export function isWorkshopModel(name: string): boolean {
  return name.startsWith(WORKSHOP_MODEL_PREFIX);
}

/** 某个下载项是不是运行时组件（不参与任何模型列表） */
export function isRuntimeComponent(name: string): boolean {
  return RUNTIME_COMPONENTS.has(name);
}

/**
 * 按分区筛下载项；保持入参顺序（列表顺序即 UI 顺序）。
 * 运行时组件一律剔除 —— 调用方不必再各自过滤一次。
 */
export function modelsInArea<T extends { name: string }>(list: readonly T[], area: ModelArea): T[] {
  return list.filter(
    (d) => !isRuntimeComponent(d.name) && isWorkshopModel(d.name) === (area === "workshop"),
  );
}
