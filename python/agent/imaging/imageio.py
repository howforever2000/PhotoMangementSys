"""文件读写与字体解析。

## 为什么保存要传文件对象而不是路径

Pillow 在 Windows 中文/非 ASCII 路径下传入**字符串路径**时，某些版本会静默写出
0 字节文件（docs/v5.md §21 明确列为坑）。所以这里统一：
    with open(dst, "wb") as f: img.save(f, format="JPEG", ...)
文件对象走的是标准 IO，不依赖 Pillow 内部的路径编码。

## 字体

卡片上的中文必须由代码画（不许让生成模型画字，否则出乱码）。
本模块按以下顺序找字体，**找到哪个就记在结果里**，找不到就明确降级并记 assumption：
    1. `assets/fonts/*.ttf|otf|ttc`（随包分发，开源字体）
    2. 系统字体（Windows 的微软雅黑/黑体/宋体）
    3. Pillow 内置位图字体（不含中文，只用于极端兜底）
"""
from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image, ImageFont, UnidentifiedImageError

# 允许的后缀（ttc 是字体集合，雅黑就是 ttc）
FONT_SUFFIXES = (".ttf", ".otf", ".ttc")

# 系统兜底字体（按"能显示中文且观感可接受"排序）
_SYSTEM_FONTS = (
    r"C:\Windows\Fonts\msyh.ttc",      # 微软雅黑
    r"C:\Windows\Fonts\msyhl.ttc",     # 微软雅黑 Light
    r"C:\Windows\Fonts\simhei.ttf",    # 黑体
    r"C:\Windows\Fonts\simsun.ttc",    # 宋体
    r"C:\Windows\Fonts\Deng.ttf",      # 等线
)


class ImageIoError(RuntimeError):
    """图片读不了 / 写不出（带人话文案）。"""


def assets_dir() -> Path:
    """随包资源目录（PyInstaller 单文件运行时在 _MEIPASS 下）。"""
    if getattr(sys, "frozen", False):
        return Path(getattr(sys, "_MEIPASS", Path(sys.executable).parent)) / "assets"
    return Path(__file__).resolve().parents[2] / "assets"


def find_font() -> tuple[str, str]:
    """返回 (字体路径, 来源说明)。找不到任何字体时路径为空。"""
    bundled = assets_dir() / "fonts"
    if bundled.is_dir():
        for p in sorted(bundled.iterdir()):
            if p.suffix.lower() in FONT_SUFFIXES:
                return str(p), f"bundled:{p.name}"
    for cand in _SYSTEM_FONTS:
        if Path(cand).is_file():
            return cand, f"system:{Path(cand).name}"
    return "", "none"


def load_font(size: int) -> tuple[ImageFont.FreeTypeFont | ImageFont.ImageFont, str]:
    """按字号取字体，并返回来源说明（写进渲染结果，便于排障"为什么卡片上没字"）。"""
    path, source = find_font()
    if path:
        try:
            return ImageFont.truetype(path, size), source
        except OSError:
            pass
    return ImageFont.load_default(), "fallback-bitmap"


def open_rgb(src: str) -> Image.Image:
    """打开并归一化为 RGB（透明底铺白；EXIF 方向先摆正）。"""
    try:
        img = Image.open(src)
        img.load()
    except FileNotFoundError as e:
        raise ImageIoError(f"图片不存在：{src}") from e
    except UnidentifiedImageError as e:
        raise ImageIoError(f"无法识别的图片格式：{src}") from e
    except OSError as e:
        raise ImageIoError(f"图片读取失败：{src}（{e}）") from e

    img = _apply_exif_orientation(img)
    if img.mode in ("RGBA", "LA", "P"):
        rgba = img.convert("RGBA")
        background = Image.new("RGB", rgba.size, (255, 255, 255))
        background.paste(rgba, mask=rgba.split()[-1])
        return background
    return img.convert("RGB")


def _apply_exif_orientation(img: Image.Image) -> Image.Image:
    """按 EXIF Orientation 摆正（拍竖图不摆正会让卡片版面错乱）。"""
    try:
        from PIL import ImageOps

        return ImageOps.exif_transpose(img)
    except Exception:  # noqa: BLE001 —— EXIF 异常不该让整张图失败
        return img


def save_jpeg(img: Image.Image, dst: str, quality: int = 90) -> int:
    """写 JPEG（传文件对象，避免中文路径静默 0 字节）。返回字节数。"""
    Path(dst).parent.mkdir(parents=True, exist_ok=True)
    with open(dst, "wb") as f:
        img.save(f, format="JPEG", quality=int(quality), optimize=True, progressive=False)
    size = Path(dst).stat().st_size
    if size <= 0:
        raise ImageIoError(f"写入 0 字节文件（Pillow 保存失败）：{dst}")
    return size


def save_png(img: Image.Image, dst: str) -> int:
    Path(dst).parent.mkdir(parents=True, exist_ok=True)
    with open(dst, "wb") as f:
        img.save(f, format="PNG", optimize=True)
    size = Path(dst).stat().st_size
    if size <= 0:
        raise ImageIoError(f"写入 0 字节文件（Pillow 保存失败）：{dst}")
    return size


def save_pdf(pages: list[Image.Image], dst: str, dpi: int = 150) -> int:
    """多页 PDF：第一页 save_all + append_images（Pillow 原生，不需要额外依赖）。"""
    if not pages:
        raise ImageIoError("没有内容可导出 PDF")
    Path(dst).parent.mkdir(parents=True, exist_ok=True)
    with open(dst, "wb") as f:
        pages[0].save(
            f,
            format="PDF",
            save_all=True,
            append_images=list(pages[1:]),
            resolution=float(dpi),
        )
    size = Path(dst).stat().st_size
    if size <= 0:
        raise ImageIoError(f"写入 0 字节文件（Pillow 保存失败）：{dst}")
    return size


def make_thumb(src_or_img: str | Image.Image, dst: str, edge: int = 160) -> str:
    """生成轨迹用的缩略图（长边 edge），返回路径。"""
    img = open_rgb(src_or_img) if isinstance(src_or_img, str) else src_or_img
    w, h = img.size
    scale = min(1.0, edge / max(w, h))
    if scale < 1.0:
        img = img.resize((max(1, int(w * scale)), max(1, int(h * scale))), Image.LANCZOS)
    save_jpeg(img, dst, quality=80)
    return dst
