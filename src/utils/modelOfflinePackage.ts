/**
 * 离线模型包（model.zip）的**唯一事实源** —— BUG-2026-1119-002
 *
 * 修前有两份逐字重复的常量：`settings/ModelDownloadsSection.vue` 与
 * `settings/ModelMissingDialog.vue` 各写一遍 URL 与「约 529MB」，于是三处口径互不相等：
 *   · UI 两处：`releases/download/v0.4.5/model.zip` + 「529MB」
 *   · README：同 URL 但「856MB」
 *   · Rust `model_dl::specs()` 实算：识别链路 **979.2MB**
 * 529MB 是 v0.4.2 的值（`feature-notes.json` 记「model.zip（v0.4.2，528.9MB）」），
 * v0.4.5 换 fp32 语义档后没跟着改 —— 典型的「同一个数字抄了两份、改了一处」。
 *
 * 本模块只放「描述离线包这个发布资产」的事实，不放任何视图逻辑：
 * 体积、URL、解压落点、装了什么。视图层（设置页 / 缺模型弹窗）只负责渲染它。
 *
 * ## 为什么 URL 用 `releases/latest/download`
 *
 * 与 OpenVINO 组件同一个口径（`src-tauri/src/model_dl.rs` 的 `openvino-runtime`）。
 * 写死 `v0.4.5` 的后果是：发了 5.0 之后这一行没人改，用户点「离线包」下到的是
 * 0.4.5 的旧包（内含已下线的 fp16 语义档，装上去语义照样不可用），而且不会报错。
 * 稳定别名把「发版必须同步改 URL」这个人肉环节从流程里删掉了。
 *
 * ## 体积为什么是「解压后」
 *
 * zip 的压缩后体积取决于打包时的压缩级别，代码无法从模型清单推出 —— 写死一个
 * 压缩后数字就会再次漂移。这里改为由 `specs()` 里 `expect_bytes` 之和推算
 * **解压后体积**（979MB），并明确标注「解压后」，用户对得上下载进度条上的总量。
 */

import { openExternalUrl } from "./openExternal.ts";

/** 离线包下载地址（GitHub Release 稳定别名，不随版本号改） */
export const OFFLINE_PACKAGE_URL =
  "https://github.com/howforever2000/PhotoMangementSys/releases/latest/download/model.zip";

/**
 * 离线包解压后的模型体积（十进制 MB，与 `ModelDownloadRows.vue` 的 `fmtMB`
 * 同口径 `bytes / 1e6`）。
 *
 * 来源：`src-tauri/src/model_dl.rs::specs()` 里识别链路 5 项 `expect_bytes` 之和
 * = 979,232,127 字节；与 `python/models/` 磁盘实算（979,259,539 字节）相差 27KB
 * （只有 `yolov8n-det.onnx` 的 expect_bytes 与实测文件有 27,412 字节偏差）。
 *
 * ⚠ 改动 `model_dl::specs()` 的识别链路模型后必须同步本常量 —— 这是唯一的
 * 「代码与文案会漂移」的地方，因此在这里显式写下推导依据，而不是留一个裸数字。
 */
export const OFFLINE_PACKAGE_UNPACKED_MB = 979;

/** 离线包覆盖的能力（展示用，顺序即用户心智里的重要性） */
export const OFFLINE_PACKAGE_CONTENTS: readonly string[] = [
  "语义模型 B/16（fp32 单档，754MB）",
  "人脸两档（det_10g + w600k_r50 / det_500m + w600k_mbf）",
  "人物检测 YOLOv8n（必需，缺它扫描链路不就绪）",
  "OCR 检测（PaddleOCR v4）",
];

/**
 * 安装形态决定「解压到哪里」—— 判据是 `vision::resolve_model_dir()` 的分支，
 * 不是「猜用户怎么装的」。
 *
 * 三种落点（`src-tauri/src/vision.rs`）：
 *   ① `resource_dir/vcr/models` 存在且**可写** → 直接用它（NSIS 按用户安装）
 *   ② 存在但**只读** → 启动时硬链接/复制到 `%APPDATA%\<identifier>\vcr-models` 并用副本（MSI）
 *   ③ **不存在** → 直接落 `%APPDATA%\<identifier>\vcr-models`（轻量包 / 无模型包）
 *
 * 修前文案只说「解压到安装根目录（MSI 只读安装为 %APPDATA%\…\vcr-models）」，
 * 漏掉分支 ③ —— 而分支 ③ 恰恰是**最常见**的情形（0.4.x 起发行包就是不含模型的
 * 轻量形态）。轻量包用户照文案解压到安装根目录，`vcr/models` 目录本来就不存在，
 * 应用启动时 `bundled.is_dir()` 为 false 直接跳过，模型永远读不到，且不报错。
 */
export interface OfflinePackageTarget {
  /** 落点标识：slim = 无内置模型目录的轻量包 */
  key: "portable" | "msi" | "slim";
  /** 一行标题 */
  title: string;
  /** 具体的解压目标路径说明 */
  path: string;
  /** 为什么是这个路径（判据，供用户自查） */
  why: string;
}

/** Windows 上 app_data 的稳定写法（与 `tauri.conf.json` 的 identifier 一致） */
const APP_MODELS_DIR = "%APPDATA%\\com.photomanagementsys.app\\vcr-models";

export const OFFLINE_PACKAGE_TARGETS: readonly OfflinePackageTarget[] = [
  {
    key: "slim",
    title: "轻量包 / 不含模型的安装包（最常见）",
    path: `${APP_MODELS_DIR}\\`,
    why: "安装目录里没有 vcr\\models，应用会直接读这里",
  },
  {
    key: "portable",
    title: "NSIS 按用户安装（可写）",
    path: "<安装根目录>\\",
    why: "zip 内层自带 vcr\\models\\，解压后即被内置资源目录直接采用",
  },
  {
    key: "msi",
    title: "MSI 装到 Program Files（只读）",
    path: `${APP_MODELS_DIR}\\`,
    why: "内置目录只读，应用首次启动会把模型硬链接到这个副本目录",
  },
];

/** zip 内层的目录结构（前缀提醒：包内已是 vcr/models/…，不要再多套一层） */
export const OFFLINE_PACKAGE_ARCHIVE_ROOT = "vcr\\models\\";

/** 「约 979MB（解压后）」—— 两处 UI 共用同一句，避免再出现两个体积 */
export function offlinePackageSizeText(): string {
  return `约 ${OFFLINE_PACKAGE_UNPACKED_MB}MB（解压后）`;
}

/**
 * 打开离线包下载页。
 *
 * 放在这里而不是各视图各写一遍 `import("@tauri-apps/plugin-opener")`：
 * 两处都要调，再各写一份就又回到「两份实现会漂移」的老路上。
 * 返回值只表达「尝试过打开」，失败由调用方决定怎么提示（各视图的 notify 不同）。
 */
export async function openOfflinePackagePage(): Promise<void> {
  await openExternalUrl(OFFLINE_PACKAGE_URL);
}

/**
 * 打开模型目录（`vocab` 之外的第二个自救入口：用户下完 zip 不想自己拼路径）。
 *
 * 目录用后端上报的真实值（`list_model_downloads` 不含路径，故由 `ai_get_*`
 * 之外的 `models_dir` 命令提供），拿不到就抛错让调用方提示，不猜路径。
 */
export async function openModelsDir(dir: string): Promise<void> {
  if (!dir) throw new Error("模型目录未知");
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("open_folder", { path: dir });
}
