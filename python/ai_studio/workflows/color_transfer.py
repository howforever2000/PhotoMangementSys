"""参考图颜色迁移工作流：把参考图的色调 / 影调迁到每张素材图，输出 JPG。

纯本机、零网络、零付费。算法在 colortransfer.py（Reinhard / 直方图匹配），
可选把这次迁移固化成 .cube 方便以后当滤镜复用。

两个性能纪律（都是实测出来的）：

1. **参考图只取统计，不按原分辨率转 Lab**。Reinhard / 直方图匹配只用 ref 的
   Lab 均值、标准差与分位分布，这些量与分辨率无关；而 24MP 转一次 Lab 要 2145ms。
   参考图降到长边 `_REF_LONG_EDGE` 后转，同样的统计量只要 24ms。
2. **照片级并行**（纯 CPU，GIL 释放充分），整批接近线性。
"""
from __future__ import annotations

import os

import numpy as np
from PIL import Image

from .. import colortransfer
from .base import filter_inputs, read_image, save_image, stem_of
from .batch import Outcome, run_batch

_ONNX_HEAVY = False

# 参考图分析用的长边：统计量与分辨率无关，1024 已足够稳定（0.19MP 转 Lab 仅 24ms；
# 24MP 是 2145ms）。直方图匹配的分位精度在 1MP 上误差已是千分级。
_REF_LONG_EDGE = 1024

# 多参考图上限：共性合并是等权混池，参考图越多统计越被稀释，10 张已是实际可用上限。
_MAX_REFS = 10

# 参考图间 a/b 均值距离的告警阈值（Lab 色度分量，近似 ΔE）：超过即提示「共性可能不明显」。
_DIVERGENCE_WARN = 20.0


def _ref_paths_from(opts: dict) -> list[str]:
    """从 options 解析参考图路径列表：`referencePaths`（数组）优先，回退 `referencePath`（单张，旧契约）。"""
    raw = opts.get("referencePaths")
    if isinstance(raw, str):
        raw = [raw]
    paths: list[str] = []
    for p in raw or []:
        s = str(p).strip()
        if s:
            paths.append(s)
    if not paths:
        legacy = str(opts.get("referencePath") or "").strip()
        if legacy:
            paths = [legacy]
    # 去重保序：同一张图选两次没有意义，只会稀释统计
    seen, uniq = set(), []
    for p in paths:
        if p not in seen:
            seen.add(p)
            uniq.append(p)
    return uniq


def _load_refs(paths: list[str]) -> list[np.ndarray]:
    """逐张解码并降到长边 `_REF_LONG_EDGE`（统计量与分辨率无关）。"""
    return [np.asarray(_fit_ref(read_image(p)), dtype=np.uint8) for p in paths]


def _merge_ref_artifacts(ref_arrs: list[np.ndarray], warnings: list[str]) -> tuple[dict, list]:
    """参考图统计归并：单张直取；多张等权合并 + 差异过大告警。"""
    stats_list = [colortransfer.reference_stats(a) for a in ref_arrs]
    merged = colortransfer.merge_reference_stats(stats_list)
    if len(stats_list) > 1:
        d = colortransfer.stats_divergence(stats_list)
        if d > _DIVERGENCE_WARN:
            warnings.append(
                f"各参考图色调差异较大（色偏相差约 {d:.0f}），合并出的共性可能不明显，建议挑色调相近的参考图"
            )
    return merged, colortransfer.histogram_ref_sorted(ref_arrs)


def _fit_ref(img: Image.Image) -> Image.Image:
    """参考图缩到长边 `_REF_LONG_EDGE`（只缩不放）。"""
    w, h = img.size
    m = max(w, h)
    if m <= _REF_LONG_EDGE:
        return img
    s = _REF_LONG_EDGE / float(m)
    return img.resize((max(1, int(round(w * s))), max(1, int(round(h * s)))), Image.LANCZOS)


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    opts = req.options or {}
    method = str(opts.get("method", "reinhard") or "reinhard")
    strength = float(opts.get("strength", 1.0) or 1.0)
    l_weight = float(max(0.0, min(1.0, float(opts.get("lWeight", 1.0) or 1.0))))
    export_cube = bool(opts.get("exportCube", False))

    warnings: list[str] = []
    ref_paths = _ref_paths_from(opts)
    if not ref_paths:
        raise ValueError("未选择参考图")
    if len(ref_paths) > _MAX_REFS:
        warnings.append(f"参考图超过 {_MAX_REFS} 张，只取前 {_MAX_REFS} 张参与共性合并")
        ref_paths = ref_paths[:_MAX_REFS]
    missing = [p for p in ref_paths if not os.path.isfile(p)]
    if missing:
        raise FileNotFoundError(f"参考图不存在: {missing[0]}")
    # 参考图只在这一处解码一次（旧版每张图都全量转 Lab，同一张 ref 被算 N 次）。
    ref_arrs = _load_refs(ref_paths)
    ref_stats, ref_sorted = _merge_ref_artifacts(ref_arrs, warnings)

    req.paths = filter_inputs(list(req.paths), warnings)

    def process(index: int, p: str) -> Outcome:
        arr = np.asarray(read_image(p), dtype=np.uint8)
        if method == "histogram":
            out_arr = colortransfer.histogram_match_sorted(arr, ref_sorted, strength=strength)
        else:
            out_arr = colortransfer.reinhard_transfer_stats(arr, ref_stats, l_weight=l_weight, strength=strength)
        out = save_image(Image.fromarray(out_arr), out_dir, f"{stem_of(p)}_迁移", ".jpg", 95)
        return Outcome(outputs=[out])

    res = run_batch(
        req.paths,
        process,
        progress,
        cancel,
        stage="颜色迁移中",
        onnx_heavy=_ONNX_HEAVY,
        initial_warnings=warnings,
    )

    outputs = list(res.outputs)
    if export_cube:
        try:
            text = colortransfer.build_cube_text_from_stats(ref_stats, size=33, l_weight=l_weight)
            suffix = stem_of(ref_paths[0]) if len(ref_paths) == 1 else f"共{len(ref_paths)}张"
            cube_path = os.path.join(out_dir, f"参考图调色_{suffix}.cube")
            with open(cube_path, "w", encoding="utf-8") as fh:
                fh.write(text)
            outputs.append(cube_path)
        except Exception as e:  # noqa: BLE001 —— 导出失败不应让整批作业失败
            res.warnings.append(f"导出 .cube 失败: {e}")

    progress(100, "完成", f"{len(res.outputs)} 张")
    return {
        "outputs": outputs,
        "warnings": res.warnings,
        "meta": {"method": method, "references": len(ref_paths)},
    }
