"""压缩：质量阶梯 + 缩边 + 居中裁切（凑目标体积）。

为什么算法长这样（docs/v5.md §9、§21）：
- **质量阶梯**而不是二分：档位固定 → 同输入同输出（可复现），也便于和 Rust 侧
  实现做对照测试；
- **凑不到就缩边**：横图 6000×4000 想要 200KB，光降质量会糊成一团；缩到
  合适尺寸再降质量观感更好；
- 整个过程只在内存里试（BytesIO），最后一次落盘 —— 避免写出几十个中间文件。

⚠️ 这套逻辑在阶段 3 会收口成**唯一实现**（Rust `tools.rs` 只留命令壳）。
在那之前两边各有一份，必须有过对照测试，别让两套在"未达标"边界上给出不同结论。
"""
from __future__ import annotations

import io
from dataclasses import dataclass
from pathlib import Path

from PIL import Image

from .imageio import ImageIoError, open_rgb, save_jpeg

# 质量阶梯（从高到低）。固定档位 = 可复现。
QUALITY_LADDER: tuple[int, ...] = (92, 88, 84, 80, 72, 64, 56, 48, 40, 32)
# 每轮缩边比例与最多缩几轮（6000px → 约 2000px 需要 5 轮）
SHRINK_FACTOR = 0.88
MAX_SHRINK_STEPS = 8
# 低于这个尺寸就不再缩（再缩就糊到没法看了，宁可超体积并如实上报）
MIN_EDGE = 320


@dataclass
class CompressResult:
    src: str
    out: str
    bytes: int
    width: int
    height: int
    quality: int
    ok: bool
    note: str = ""

    def to_dict(self) -> dict:
        return {
            "src": self.src,
            "out": self.out,
            "bytes": self.bytes,
            "w": self.width,
            "h": self.height,
            "quality": self.quality,
            "ok": self.ok,
            "note": self.note,
        }


def parse_crop(crop: str) -> float | None:
    """`"4:3"` → 4/3。空串或非法值返回 None（不裁切）。"""
    if not crop:
        return None
    try:
        w, h = crop.split(":")
        rw, rh = float(w), float(h)
        if rw <= 0 or rh <= 0:
            return None
        return rw / rh
    except (ValueError, AttributeError):
        return None


def center_crop(img: Image.Image, ratio: float | None) -> Image.Image:
    """按目标宽高比居中裁切（只裁不减，不改变内容位置语义）。"""
    if not ratio:
        return img
    w, h = img.size
    cur = w / h
    if abs(cur - ratio) < 1e-3:
        return img
    if cur > ratio:  # 太宽 → 裁两边
        new_w = max(1, int(round(h * ratio)))
        left = (w - new_w) // 2
        return img.crop((left, 0, left + new_w, h))
    new_h = max(1, int(round(w / ratio)))
    top = (h - new_h) // 2
    return img.crop((0, top, w, top + new_h))


def fit_edge(img: Image.Image, max_edge: int) -> Image.Image:
    """长边限制（只缩不放）。"""
    if max_edge <= 0:
        return img
    w, h = img.size
    longest = max(w, h)
    if longest <= max_edge:
        return img
    scale = max_edge / longest
    return img.resize((max(1, int(round(w * scale))), max(1, int(round(h * scale)))), Image.LANCZOS)


def _encode_size(img: Image.Image, quality: int) -> int:
    """在内存里试编一次，返回字节数（不落盘）。"""
    buf = io.BytesIO()
    img.save(buf, format="JPEG", quality=int(quality), optimize=True)
    return buf.tell()


def compress(
    src: str,
    dst: str,
    *,
    target_kb: int = 0,
    max_edge: int = 0,
    crop: str = "",
    quality: int = 90,
) -> CompressResult:
    """单张压缩。`target_kb<=0` 表示只做尺寸/质量归一，不凑体积。"""
    img = open_rgb(src)
    img = center_crop(img, parse_crop(crop))
    img = fit_edge(img, max_edge)

    if target_kb <= 0:
        size = save_jpeg(img, dst, quality=quality)
        return CompressResult(src, dst, size, img.width, img.height, quality, True)

    target_bytes = int(target_kb) * 1024
    best: tuple[int, int, int, bool] | None = None  # (bytes, quality, edge, ok)

    edge = max(img.width, img.height)
    work = img
    for step in range(MAX_SHRINK_STEPS + 1):
        for q in QUALITY_LADDER:
            size = _encode_size(work, q)
            if size <= target_bytes:
                # 命中：在同尺寸下取**最高可用质量**（内层从高到低，第一次命中即最优）
                best = (size, q, max(work.width, work.height), True)
                break
        if best:
            break
        # 这一轮全部档位都超标 → 缩边再来
        if step == MAX_SHRINK_STEPS or max(work.width, work.height) <= MIN_EDGE:
            break
        new_w = max(1, int(round(work.width * SHRINK_FACTOR)))
        new_h = max(1, int(round(work.height * SHRINK_FACTOR)))
        if max(new_w, new_h) < MIN_EDGE:
            break
        work = work.resize((new_w, new_h), Image.LANCZOS)

    if best is None:
        # 凑不到：用最低档落盘，并**如实上报**（不假装成功）
        q = QUALITY_LADDER[-1]
        size = save_jpeg(work, dst, quality=q)
        return CompressResult(
            src,
            dst,
            size,
            work.width,
            work.height,
            q,
            False,
            note=f"已压到最低档仍超过目标 {target_kb}KB（实得 {size // 1024}KB）",
        )

    size, q, _edge, _ok = best
    size = save_jpeg(work, dst, quality=q)
    return CompressResult(src, dst, size, work.width, work.height, q, True)


def output_name(src: str, out_dir: str, suffix: str = "_c") -> str:
    """产物路径：同目录下 `<stem><suffix>.jpg`（不覆盖原图）。"""
    stem = Path(src).stem
    return str(Path(out_dir) / f"{stem}{suffix}.jpg")


__all__ = [
    "compress",
    "CompressResult",
    "parse_crop",
    "center_crop",
    "fit_edge",
    "output_name",
    "ImageIoError",
]
