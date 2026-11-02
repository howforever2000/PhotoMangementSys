"""产物体检：尺寸 / 体积 / 清晰度。

为什么值得有：Agent 交付前必须能自证"成果没坏"。一张 0 字节或糊成马赛克的图，
如果只看"文件存在"就报成功，用户会在打开时才发现 —— 那时已经不知道是哪一步坏的。
"""
from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageFilter, ImageStat

from .imageio import ImageIoError


def sharpness(path: str) -> float:
    """清晰度代理值：边缘检测后的方差（越大越锐）。

    不用 numpy：`ImageFilter.FIND_EDGES` + `ImageStat.Stat(...).var` 就够了，
    而且对"明显糊掉"足够敏感（同一张图模糊后该值会掉一个数量级）。
    """
    img = Image.open(path).convert("L")
    # 统一到小尺寸再算：避免大图算得慢，也让不同尺寸的结果可比
    img.thumbnail((512, 512))
    edges = img.filter(ImageFilter.FIND_EDGES)
    var = ImageStat.Stat(edges).var[0]
    return round(float(var), 3)


def inspect(path: str) -> dict:
    """产物体检。文件不存在或 0 字节都算错（返回 ok=False，不抛）。

    PDF 单独分支：Pillow 不能解 PDF（也没必要为它引入 ghostscript），
    只校验魔数与体积。旧实现直接 `Image.open(pdf)` 会失败，导致
    “卡片明明生成了却报所有产物都不合格”。
    """
    p = Path(path)
    if not p.is_file():
        return {"path": path, "ok": False, "error": "文件不存在"}
    size = p.stat().st_size
    if size <= 0:
        return {"path": path, "ok": False, "error": "0 字节文件"}

    if p.suffix.lower() == ".pdf":
        with open(p, "rb") as f:
            magic = f.read(4)
        if magic != b"%PDF":
            return {"path": path, "ok": False, "error": "PDF 魔数不对（文件损坏）"}
        return {
            "path": str(p),
            "ok": True,
            "bytes": size,
            "kb": round(size / 1024, 1),
            "format": "PDF",
        }

    try:
        with Image.open(p) as img:
            w, h = img.size
            fmt = img.format or ""
    except Exception as e:  # noqa: BLE001 —— 体检本身不该抛
        return {"path": path, "ok": False, "error": f"无法打开：{e}"}
    out = {
        "path": str(p),
        "ok": True,
        "bytes": size,
        "kb": round(size / 1024, 1),
        "w": w,
        "h": h,
        "format": fmt,
    }
    # 清晰度只对 JPEG/PNG 这类位图算（PDF 之类跳过）
    if fmt in ("JPEG", "PNG", "WEBP", "BMP"):
        try:
            out["sharpness"] = sharpness(str(p))
        except Exception as e:  # noqa: BLE001
            out["sharpness"] = None
            out["sharpnessError"] = str(e)
    return out


def assert_ok(path: str) -> dict:
    """体检不通过就抛（执行器用它做最后一道闸）。"""
    info = inspect(path)
    if not info.get("ok"):
        raise ImageIoError(f"产物体检未通过：{info.get('error')}（{path}）")
    return info
