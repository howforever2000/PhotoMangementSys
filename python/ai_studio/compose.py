"""手账版式合成与导出（FEAT-152）

职责单一：**只做确定性的图片排版与文件导出**，不调用任何模型。
输入已风格化好的 PIL 图片 + 主题 + 配文，输出单页 JPG 或多页 PDF。

为什么不把版式交给图像模型：中文排版由扩散模型生成几乎必然乱码/错字，
且不可控、不可复现。这里用 Pillow 按模板参数确定性地画，文字只用系统字体渲染。
"""
from __future__ import annotations

import os
from typing import List, Tuple

from PIL import Image, ImageDraw, ImageFont, ImageOps

from .presets import JOURNAL_TEMPLATES

# Windows 常见中文字体（系统字体，不打进包）：粗体优先，其次常规；最后退回默认字体
_FONT_BOLD = [
    r"C:\Windows\Fonts\msyhbd.ttc",
    r"C:\Windows\Fonts\msyh.ttc",
    r"C:\Windows\Fonts\simhei.ttf",
    r"C:\Windows\Fonts\simsun.ttc",
]
_FONT_NORMAL = [
    r"C:\Windows\Fonts\msyh.ttc",
    r"C:\Windows\Fonts\simhei.ttf",
    r"C:\Windows\Fonts\simsun.ttc",
    r"C:\Windows\Fonts\arial.ttf",
]


def _load_font(size: int, bold: bool = False) -> ImageFont.FreeTypeFont:
    for path in (_FONT_BOLD if bold else _FONT_NORMAL):
        try:
            if os.path.isfile(path):
                return ImageFont.truetype(path, size, index=0)
        except Exception:
            continue
    try:
        return ImageFont.load_default(size=size)  # Pillow ≥10 支持 size
    except Exception:
        return ImageFont.load_default()


def _fit_contain(img: Image.Image, box_w: int, box_h: int) -> Image.Image:
    w, h = img.size
    if w <= 0 or h <= 0:
        return img
    scale = min(box_w / w, box_h / h)
    nw, nh = max(1, int(w * scale)), max(1, int(h * scale))
    return img.resize((nw, nh), Image.LANCZOS)


def _fit_cover(img: Image.Image, box_w: int, box_h: int) -> Image.Image:
    w, h = img.size
    if w <= 0 or h <= 0:
        return img
    scale = max(box_w / w, box_h / h)
    nw, nh = max(1, int(w * scale)), max(1, int(h * scale))
    resized = img.resize((nw, nh), Image.LANCZOS)
    left = (nw - box_w) // 2
    top = (nh - box_h) // 2
    return resized.crop((left, top, left + box_w, top + box_h))


def _wrap(draw: ImageDraw.ImageDraw, text: str, font: ImageFont.FreeTypeFont, max_w: int) -> List[str]:
    """按像素宽度逐字折行（中文无空格，只能按字符量宽）。"""
    lines: List[str] = []
    cur = ""
    for ch in text or "":
        if ch == "\n":
            lines.append(cur)
            cur = ""
            continue
        trial = cur + ch
        try:
            w = draw.textlength(trial, font=font)
        except Exception:
            w = len(trial) * font.size
        if w <= max_w or not cur:
            cur = trial
        else:
            lines.append(cur)
            cur = ch
    if cur:
        lines.append(cur)
    return lines


def _draw_text_block(
    draw: ImageDraw.ImageDraw,
    text: str,
    font: ImageFont.FreeTypeFont,
    x: int,
    y: int,
    max_w: int,
    fill: tuple,
    line_gap: int = 12,
    max_lines: int = 6,
) -> int:
    """从 (x,y) 起画折行文本，返回结束后的 y。"""
    lines = _wrap(draw, text, font, max_w)[:max_lines]
    for ln in lines:
        draw.text((x, y), ln, font=font, fill=fill)
        try:
            lh = font.size + line_gap
        except Exception:
            lh = 40
        y += lh
    return y


def build_page(
    photo: Image.Image,
    template_id: str,
    theme: str,
    caption: str,
    index: int,
    total: int,
) -> Image.Image:
    """按模板把「一张已风格化照片 + 主题 + 配文」排成一页 A4。"""
    tpl = JOURNAL_TEMPLATES.get(template_id, JOURNAL_TEMPLATES["blank"])
    W, H = tpl["page"]
    bg = tuple(tpl["bg"])
    page = Image.new("RGB", (W, H), bg)
    draw = ImageDraw.Draw(page)
    tf = _load_font(tpl["title_size"], bold=True)
    cf = _load_font(tpl["caption_size"])

    if template_id == "magazine":
        # 上半幅满版（cover 裁切），下半幅白底文字区
        img_h = int(H * tpl["image_ratio"])
        cover = _fit_cover(photo, W, img_h)
        page.paste(cover, (0, 0))
        mx = 92
        y = img_h + 74
        if theme:
            draw.text((mx, y), theme, font=tf, fill=tuple(tpl["title_color"]))
            y += tpl["title_size"] + 26
        draw.line([(mx, y), (mx + 84, y)], fill=tuple(tpl["accent"]), width=4)
        y += 24
        _draw_text_block(draw, caption, cf, mx, y, W - 2 * mx, tuple(tpl["caption_color"]), max_lines=5)
        draw.text((W - mx - 90, H - 80), f"{index + 1} / {total}", font=cf, fill=tuple(tpl["caption_color"]))
        return page

    m = int(tpl["margin"])
    inner_w = W - 2 * m
    y = m
    if theme:
        draw.text((m, y), theme, font=tf, fill=tuple(tpl["title_color"]))
        y += tpl["title_size"] + 22
        draw.line([(m, y), (m + 76, y)], fill=tuple(tpl["accent"]), width=3)
        y += 26

    img_h = int(H * tpl["image_ratio"])
    fitted = _fit_contain(photo, inner_w, img_h)
    ix = m + (inner_w - fitted.size[0]) // 2
    page.paste(fitted, (ix, y))
    if tpl["frame"]:
        draw.rectangle(
            [ix - 6, y - 6, ix + fitted.size[0] + 6, y + fitted.size[1] + 6],
            outline=tuple(tpl["accent"]),
            width=3,
        )
    y += fitted.size[1] + 44

    _draw_text_block(draw, caption, cf, m, y, inner_w, tuple(tpl["caption_color"]), max_lines=6)
    draw.text((W - m - 96, H - m - 10), f"{index + 1} / {total}", font=cf, fill=tuple(tpl["caption_color"]))
    return page


def _unique(path: str) -> str:
    if not os.path.exists(path):
        return path
    stem, ext = os.path.splitext(path)
    i = 1
    while os.path.exists(f"{stem}_{i}{ext}"):
        i += 1
    return f"{stem}_{i}{ext}"


def export_journal(
    items: List[Tuple[Image.Image, str]],
    template_id: str,
    theme: str,
    out_dir: str,
    stem: str,
) -> List[str]:
    """items = [(风格化照片, 配文)]。1 张 → JPG；多张 → 多页 PDF（附首图预览 JPG）。"""
    os.makedirs(out_dir, exist_ok=True)
    total = len(items)
    pages = [build_page(img, template_id, theme, cap, i, total) for i, (img, cap) in enumerate(items)]
    outputs: List[str] = []
    if total == 1:
        path = _unique(os.path.join(out_dir, f"{stem}_手账.jpg"))
        pages[0].save(path, "JPEG", quality=92)
        outputs.append(path)
        return outputs
    pdf_path = _unique(os.path.join(out_dir, f"{stem}_手账.pdf"))
    pages[0].save(pdf_path, "PDF", resolution=150.0, save_all=True, append_images=pages[1:])
    outputs.append(pdf_path)
    preview = _unique(os.path.join(out_dir, f"{stem}_手账_预览.jpg"))
    pages[0].save(preview, "JPEG", quality=92)
    outputs.append(preview)
    return outputs
