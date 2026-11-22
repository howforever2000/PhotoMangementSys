"""人像形变工作流：自动瘦脸 + 大眼 + 眼睛提亮，本机推理，输出 JPG。

纯本机、零费用。链路：读图 → SCRFD 检出人脸（可能多张）→ 68 点关键点
→ 眼睛提亮 → 逐脸液化 → 落盘。检测/关键点/提亮/液化的编排与配方链路共用
`ops_local.reshape_image`（本文件不再自己写一份循环与降级逻辑，参数不会漂移）。

降级纪律（沿用 FEAT-164 / FEAT-165，见 `knowledge/local-enhance.md` §6）：
  - 模型缺失 → 明确报错（可操作提示），因为这是工具的**唯一**依赖；
  - 单张图检不到人脸 / 置信度不足 / 形变异常 → 该张原样输出 + warning，不整批失败；
  - 多张脸时逐张处理，某张脸失败不影响同一张图上的其它脸。

强度口径（FEAT-210）：本工具是**手动**路径，强度严格按用户所选执行（不按发长折算）；
配方/工作台的自动档才按发长收敛（男性约为滑条值的 2/3）。

顺序纪律：本工具必须**先于**本地美颜（磨皮）执行。磨皮的皮肤掩码是按人脸框在
原图上算出来的（`workflows/local_beauty.py`），形变后再磨会「掩码与像素对不上」，
表现是皮肤没磨到、背景被磨。

并行：照片级但按「ONNX 链路」折半取线程（每个会话自己还有 intra_op 线程，
两层叠满会超订；实测 ORT 并发只有 1.66x）。推理本身仍受 `gpu_lock` 串行，
排队期间其它线程的解码/形变 numpy 运算照常推进。
"""
from __future__ import annotations

import os

from PIL import Image

from .. import landmark, localmodels, ops_local
from .base import filter_inputs, read_image, save_image, stem_of
from .batch import Outcome, run_batch

# 默认强度：与配方/工作台同一口径（FEAT-210「自然」档：瘦脸 15%、大眼 20%、眼睛提亮 10%）
_DEFAULT_SLIM = 0.15
_DEFAULT_EYES = 0.20
_DEFAULT_EYE_BRIGHT = 0.10

_ONNX_HEAVY = True


def _ensure_models() -> None:
    if not localmodels.face_det_ready():
        raise FileNotFoundError(
            "人脸检测模型未就绪（det_10g.onnx），请先在「设置 › 模型管理」下载「人脸 · 高精度」"
        )
    if not landmark.ready():
        raise FileNotFoundError(
            "人脸关键点模型未就绪（peppa_wutz.onnx），请在创意工坊「本地模型」里下载后重试"
        )


def _reshape_one(
    img: Image.Image, slim: float, eyes: float, eye_bright: float, name: str
) -> tuple[Image.Image, int, list[str]]:
    """对单张图做形变；出任何问题都返回原图（不抛）。返回 (图, 成功脸数, warnings)。"""
    warnings: list[str] = []
    stats = {"faces": 0}
    out = ops_local.reshape_image(
        img,
        slim,
        eyes,
        eye_bright,
        warn=lambda msg: warnings.append(f"{name}: {msg}"),
        stats=stats,
        auto_gender=False,  # 手动工具：用户选多强就是多强
    )
    return out, stats["faces"], warnings


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    opts = req.options or {}
    try:
        slim = float(opts.get("slim", _DEFAULT_SLIM))
    except Exception:
        slim = _DEFAULT_SLIM
    try:
        eyes = float(opts.get("eyes", _DEFAULT_EYES))
    except Exception:
        eyes = _DEFAULT_EYES
    try:
        eye_bright = float(opts.get("eye_bright", _DEFAULT_EYE_BRIGHT))
    except Exception:
        eye_bright = _DEFAULT_EYE_BRIGHT
    slim = max(0.0, min(0.8, slim))
    eyes = max(0.0, min(0.5, eyes))
    eye_bright = max(0.0, min(1.0, eye_bright))

    _ensure_models()

    warnings: list[str] = []
    req.paths = filter_inputs(list(req.paths), warnings)

    def process(index: int, path: str) -> Outcome:
        img = read_image(path)
        result, faces, w = _reshape_one(
            img, slim, eyes, eye_bright, os.path.basename(path)
        )
        out = save_image(result, out_dir, f"{stem_of(path)}_形变", ".jpg", 95)
        return Outcome(outputs=[out], warnings=w, meta={"images": 1, "faces": faces})

    stats = {"images": 0, "faces": 0}
    res = run_batch(
        req.paths,
        process,
        progress,
        cancel,
        stage="人像形变中",
        onnx_heavy=_ONNX_HEAVY,
        initial_warnings=warnings,
        stats=stats,
    )

    progress(100, "完成", f"{len(res.outputs)} 张")
    return {
        "outputs": res.outputs,
        "warnings": res.warnings,
        "meta": {
            "slim": round(slim, 4),
            "eyes": round(eyes, 4),
            "eye_bright": round(eye_bright, 4),
            **stats,
        },
    }
