"""本地美颜（磨皮 + 肤色保真）工作流：SCRFD 找人脸 → BiSeNet 分区 → 只磨皮肤
→ 羽化贴回 → 肤色朝原图锚点拉回并叠加白黄·嫩红偏置（FEAT-208），逐脸按发长侧重
（FEAT-209：长发更白更嫩红，其余还原原肤色 + 轻提亮）。

纯本机、零付费。两个模型：`det_10g.onnx`（扫描入库已有，无需下载）与
`face_parsing_bisenet_r34.onnx`（应用内下载，约 93MB）。

设计纪律：
  - 检不到人脸 / 模型缺失 → 记 warning 继续下一张，**绝不整批失败**（与 FEAT-164 一致）；
  - 检测或解析失败时该张原样输出并告警，不产出「半张糊掉」的结果；
  - 逐张的检测框外扩边距、过小脸阈值都走 `ops_local` 的单一真相源
    （本文件不再自己写一份 detect→parse→smooth 循环，避免两份参数逐渐漂移）；
  - `_MIN_FACE_PX` 判的是**原图像素**（不是预览缩放后的比例），所以预览链路也不能
    先把图缩了再进来（见 `preview.py` 的同类说明）。

肤色复用纪律（FEAT-208/209）：本工具没有调色/滤镜在前面，输入图就是原图 ——
磨皮已经跑过的那份人脸分析（含发长）直接用作肤色锚点与应用掩码，**零重复推理**。

并行：照片级但按「ONNX 链路」折半取线程 —— 每张图要跑 1 次 SCRFD + 每张脸 1 次
BiSeNet，会话自身已有 intra_op 线程，两层叠满会超订。
"""
from __future__ import annotations

import os

import numpy as np
from PIL import Image

from .. import faceparse, localmodels, ops_local, skintone
from .base import filter_inputs, read_image, save_image, stem_of
from .batch import Outcome, run_batch

_ONNX_HEAVY = True


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    opts = req.options or {}
    # 用 `is None` 判缺省，不用 `or`：`or` 会把 0 当缺省吞掉，
    # 导致「磨皮拖到 0 = 关闭」在界面上无效（BUG-2026-1117-001）。
    strength = _opt_float(opts, "strength", 0.45)
    keep_texture = _opt_float(opts, "texture", 0.55)
    # 肤色保真（FEAT-208）：四项全 0 = 关闭；缺省走「自然」档（与前端默认档一致）
    _skin_defaults = skintone.SkinParams()
    skin_params = {
        "hue_shift": _opt_float(opts, "hue_shift", _skin_defaults.hue_shift),
        "chroma": _opt_float(opts, "chroma", _skin_defaults.chroma),
        "brighten": _opt_float(opts, "brighten", _skin_defaults.brighten),
        "blush": _opt_float(opts, "blush", _skin_defaults.blush),
    }

    if not localmodels.face_det_ready():
        raise FileNotFoundError(
            "人脸检测模型未就绪（det_10g.onnx），请先在「设置 › 模型管理」下载「人脸 · 高精度」"
        )
    if not faceparse.ready():
        raise FileNotFoundError(
            "人脸解析模型未就绪（face_parsing_bisenet_r34.onnx），"
            "请在创意工坊「本地增强模型」里下载后重试"
        )

    warnings: list[str] = []
    req.paths = filter_inputs(list(req.paths), warnings)

    def process(index: int, p: str) -> Outcome:
        img = read_image(p)
        w: list[str] = []
        out_name = f"{stem_of(p)}_美颜"

        def warn(msg: str) -> None:
            w.append(f"{os.path.basename(p)}: {msg}")

        # 磨皮：检测框外扩、过小脸过滤、失败降级都在 ops_local 里，参数只有一份
        scratch: dict = {}
        smoothed = ops_local.smooth_image(
            img, strength, keep_texture, warn=warn, stats=stats, scratch=scratch
        )
        analysis = scratch.get("face_analysis") or []

        # 肤色保真（FEAT-208/209）：输入图就是原图 → 磨皮的分析（含发长）既当锚点
        # 又当应用掩码；磨皮只改皮肤纹理不改几何，掩码对磨皮输出依然对得上。
        result = np.asarray(smoothed, dtype=np.uint8)
        skin_touched = 0
        if analysis and not skintone.SkinParams.from_raw(skin_params).is_noop():
            toned = ops_local.skin_tone_image(
                smoothed,
                skin_params,
                source=img,
                faces_cur=analysis,
                faces_src=analysis,
                warn=warn,
            )
            result = np.asarray(toned, dtype=np.uint8)
            skin_touched = len(analysis)

        out = save_image(Image.fromarray(result), out_dir, out_name, ".jpg", 95)
        return Outcome(outputs=[out], warnings=w,
                        meta={"faces": len(analysis), "skin_faces": skin_touched})

    stats = {"faces": 0}
    res = run_batch(
        req.paths,
        process,
        progress,
        cancel,
        stage="美颜处理中",
        onnx_heavy=_ONNX_HEAVY,
        initial_warnings=warnings,
        stats=stats,
    )

    progress(100, "完成", f"{len(res.outputs)} 张")
    return {
        "outputs": res.outputs,
        "warnings": res.warnings,
        "meta": {
            "strength": round(strength, 4),
            "texture": round(keep_texture, 4),
            "faces": stats.get("faces", 0),
            "skin": {k: round(v, 4) for k, v in skin_params.items()},
        },
    }


def _opt_float(opts: dict, key: str, default: float) -> float:
    """取浮点选项：只有「缺省 / None / 空串」才用默认值，0 是合法值。"""
    v = opts.get(key)
    if v is None or v == "":
        return float(default)
    return float(v)
