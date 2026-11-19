"""文字提取工作流（FEAT-171）：每张图 det 找框 → rec 认字 → 落 .txt + 结果进 meta。

纯本机、零网络、零付费。模型依赖由 `op_catalog.json` 的 `ocr` 算子声明
（internal：仅供本工作流编排，planner / 配方不得直接调用）：

  - paddleocr_det：扫描区共享（模型管理「文字 · OCR 检测」，落 <models>/paddleocr-det.onnx）
  - paddleocr_rec：工坊区下载（落 <models>/local/paddleocr-rec.onnx）；
    CTC 字典 ppocr_keys_v1.txt（26KB，Apache-2.0）随包内置，不下载。

产物：`<输出目录>/<原名>_文字.txt`（UTF-8，按阅读顺序一行一行）。
识别不出文字时不是错误 —— 照片本来可能没有文字，给 warning 并跳过。

并行：照片级（ONNX 链路折半）。rec 已钉 CPU provider，不吃 GPU 串行锁，
所以 OCR 批次的并行度比磨皮/形变更高也不会互相堵。
meta.text 仍按**照片顺序**拼接（并行完成顺序乱序，不能用完成顺序）。
"""
from __future__ import annotations

import os

from .. import config, ocr_engine
from .base import filter_inputs, read_image, stem_of, unique_path
from .batch import Outcome, run_batch

TXT_SUFFIX = "_文字"

_ONNX_HEAVY = True


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

    def process(index: int, p: str) -> Outcome:
        img = read_image(p)
        lines, score, boxes = ocr_engine.recognize(img)
        if not lines:
            return Outcome(
                warnings=[f"{stem_of(p)}：未识别到文字（共检出 {boxes} 个候选区域）"]
            )
        txt_path = unique_path(out_dir, f"{stem_of(p)}{TXT_SUFFIX}", ".txt")
        with open(txt_path, "w", encoding="utf-8") as fh:
            fh.write("\n".join(lines) + "\n")
        return Outcome(outputs=[txt_path], meta={"lines": len(lines), "text": "\n".join(lines)})

    # 按照片顺序收全文（并行完成顺序乱序，必须在收尾阶段按 index 拼）
    per_photo: dict[int, list[str]] = {}

    def collect(index: int, outcome: Outcome) -> None:
        text = outcome.meta.get("text") or ""
        per_photo[index] = text.splitlines() if text else []

    res = run_batch(
        paths,
        process,
        progress,
        cancel,
        stage="识别文字中",
        onnx_heavy=_ONNX_HEAVY,
        initial_warnings=warnings,
        on_result=collect,
    )

    all_lines: list[str] = []
    for i in range(len(paths)):
        all_lines.extend(per_photo.get(i) or [])
    # 无文字的张数从「按序收上来的结果」数出来：并行下不能用完成顺序计数
    empty = sum(1 for i in range(len(paths)) if not per_photo.get(i, []))

    progress(100, "完成", f"{len(res.outputs)} 个文本")
    return {
        "outputs": res.outputs,
        "warnings": res.warnings,
        "meta": {
            "text": "\n".join(all_lines),
            "lines": len(all_lines),
            "emptyImages": empty,
        },
    }
