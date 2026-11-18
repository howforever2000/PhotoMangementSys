"""文字提取工作流（FEAT-171）：每张图 det 找框 → rec 认字 → 落 .txt + 结果进 meta。

纯本机、零网络、零付费。模型依赖由 `op_catalog.json` 的 `ocr` 算子声明
（internal：仅供本工作流编排，planner / 配方不得直接调用）：

  - paddleocr_det：扫描区共享（模型管理「文字 · OCR 检测」，落 <models>/paddleocr-det.onnx）
  - paddleocr_rec：工坊区下载（落 <models>/local/paddleocr-rec.onnx）；
    CTC 字典 ppocr_keys_v1.txt（26KB，Apache-2.0）随包内置，不下载。

产物：`<输出目录>/<原名>_文字.txt`（UTF-8，按阅读顺序一行一行）。
识别不出文字时不是错误 —— 照片本来可能没有文字，给 warning 并跳过。
"""
from __future__ import annotations

import os

from .. import config, ocr_engine
from .base import check_cancel, filter_inputs, read_image, stem_of, unique_path

TXT_SUFFIX = "_文字"


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    # 就绪检查放最前面（拉起模型前就给出可操作的报错，而不是推理时才炸）
    if not ocr_engine.det_ready():
        raise FileNotFoundError(
            "缺少 OCR 检测模型 paddleocr-det.onnx：请到「模型管理」下载「文字 · OCR 检测」后重试"
        )
    if not ocr_engine.rec_ready():
        raise FileNotFoundError(
            "缺少 OCR 识别模型 paddleocr-rec.onnx：请到「工具面板」下载「文字提取 · OCR 识别」后重试"
        )
    if not os.path.isfile(config.OCR_KEYS_PATH):
        raise FileNotFoundError("内置 CTC 字典缺失: ppocr_keys_v1.txt（安装包不完整）")

    warnings: list[str] = []
    paths = filter_inputs(list(req.paths), warnings)

    outputs: list[str] = []
    all_lines: list[str] = []
    empty = 0
    n = len(paths)
    for i, p in enumerate(paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "识别文字中", f"{i + 1}/{n}")
        img = read_image(p)
        lines, score, boxes = ocr_engine.recognize(img)
        if not lines:
            empty += 1
            warnings.append(f"{stem_of(p)}：未识别到文字（共检出 {boxes} 个候选区域）")
            continue
        txt_path = unique_path(out_dir, f"{stem_of(p)}{TXT_SUFFIX}", ".txt")
        with open(txt_path, "w", encoding="utf-8") as fh:
            fh.write("\n".join(lines) + "\n")
        outputs.append(txt_path)
        all_lines.extend(lines)
        progress(
            3 + int((i + 1) / n * 92),
            "识别文字中",
            f"{i + 1}/{n} · {len(lines)} 行 · 置信度 {score:.2f}",
        )

    progress(100, "完成", f"{len(outputs)} 个文本")
    return {
        "outputs": outputs,
        "warnings": warnings,
        "meta": {
            "text": "\n".join(all_lines),
            "lines": len(all_lines),
            "emptyImages": empty,
        },
    }
