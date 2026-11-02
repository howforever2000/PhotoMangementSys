"""确定性排版：把一组照片排成卡片页，并导出图片/PDF。

**文字由代码画，绝不让生成模型画**（docs/v5.md §21）：模型画中文会出乱码，
而且同一批卡片每次字都不一样。这里版式、字号、间距全是常量，
同一输入 → 逐像素相同的输出（可复现，可回归测试）。

`subject.mode` 的两条路：
- `contain`（默认，零成本）：照片按原比例装进版式框里，不抠主体；
- `extract`（需要远程抠图）：本版本未实现，由配方层当作 needs_key 拦住。
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageDraw

from .imageio import load_font, open_rgb, save_jpeg, save_pdf

# 页面尺寸：A4 @150dpi（1240×1754）。用像素而不是 mm：Pillow 画字按像素算。
PAGE_SIZES: dict[str, tuple[int, int]] = {
    "a4": (1240, 1754),
    "a4-landscape": (1754, 1240),
}

# 每页容纳的照片数（版式决定）
LAYOUT_CAPACITY: dict[str, int] = {
    "magazine_3up": 3,
    "grid_2up": 2,
    "polaroid": 1,
}

MARGIN = 72
HEADER_H = 134
FOOTER_H = 96
GAP = 24
PAD = 12          # 照片白框内边距
TITLE_SIZE = 46
SUBTITLE_SIZE = 24
FOOTER_SIZE = 18


@dataclass
class CardResult:
    pages: list[str]
    pdf: str | None
    page_count: int
    font_source: str
    layout: str
    images_used: int
    fitted: int  # 因版式容量/页数上限被舍弃的张数

    def to_dict(self) -> dict:
        return {
            "pages": list(self.pages),
            "pdf": self.pdf,
            "pageCount": self.page_count,
            "fontSource": self.font_source,
            "layout": self.layout,
            "imagesUsed": self.images_used,
            "imagesDropped": self.fitted,
        }


def _fit_contain(img: Image.Image, box_w: int, box_h: int) -> tuple[Image.Image, int, int]:
    """等比缩放到能装进 box，返回 (图, 左上角 x, 左上角 y)。"""
    w, h = img.size
    scale = min(box_w / w, box_h / h)
    nw, nh = max(1, int(w * scale)), max(1, int(h * scale))
    resized = img.resize((nw, nh), Image.LANCZOS) if (nw, nh) != (w, h) else img
    return resized, (box_w - nw) // 2, (box_h - nh) // 2


def _draw_photo(page: Image.Image, draw: ImageDraw.ImageDraw, src: str, box: tuple[int, int, int, int]) -> None:
    """把一个照片槽位画出来：白框 + 居中等比照片。"""
    x0, y0, x1, y1 = box
    # 白框（相纸感）：先铺白底，再往里放图
    draw.rectangle((x0, y0, x1, y1), fill=(255, 255, 255))
    inner_w, inner_h = (x1 - x0) - 2 * PAD, (y1 - y0) - 2 * PAD
    if inner_w <= 8 or inner_h <= 8:
        return
    try:
        img = open_rgb(src)
    except Exception:  # noqa: BLE001 —— 单张坏图不该毁掉整页
        draw.line((x0 + PAD, y0 + PAD, x1 - PAD, y1 - PAD), fill=(210, 205, 196), width=2)
        return
    fitted, dx, dy = _fit_contain(img, inner_w, inner_h)
    page.paste(fitted, (x0 + PAD + dx, y0 + PAD + dy))


def _boxes(layout: str, page_w: int, page_h: int) -> list[tuple[int, int, int, int]]:
    """版式几何：返回每页最多 N 个照片框。全是常量推导，无随机。"""
    left = MARGIN
    right = page_w - MARGIN
    top = MARGIN + HEADER_H
    bottom = page_h - FOOTER_H

    if layout == "polaroid":
        return [(left, top, right, bottom)]

    if layout == "grid_2up":
        mid = top + (bottom - top) // 2 - GAP // 2
        return [(left, top, right, mid), (left, mid + GAP, right, bottom)]

    # magazine_3up（默认）：左侧一张主图 + 右侧两张辅图
    main_w = int((right - left - GAP) * 0.62)
    main = (left, top, left + main_w, bottom)
    sx0 = left + main_w + GAP
    mid_y = top + (bottom - top) // 2 - GAP // 2
    small_top = (sx0, top, right, mid_y)
    small_bottom = (sx0, mid_y + GAP, right, bottom)
    return [main, small_top, small_bottom]


def render_pages(
    paths: list[str],
    *,
    layout: str = "magazine_3up",
    title: str = "",
    subtitle: str = "",
    page_size: str = "a4",
    max_pages: int = 12,
) -> tuple[list[Image.Image], str, int]:
    """渲染所有页。返回 (页面图列表, 字体来源, 实际使用张数)。"""
    capacity = LAYOUT_CAPACITY.get(layout, 3)
    page_w, page_h = PAGE_SIZES.get(page_size, PAGE_SIZES["a4"])

    chosen = list(paths)[: capacity * max(1, max_pages)]
    pages: list[Image.Image] = []
    font_src = "none"

    for start in range(0, len(chosen), capacity):
        chunk = chosen[start : start + capacity]
        page = Image.new("RGB", (page_w, page_h), (255, 255, 255))
        draw = ImageDraw.Draw(page)

        # 页眉：标题 + 副标题 + 一条细线（固定位置，不随内容变化）
        y = MARGIN
        if title:
            f_title, font_src = load_font(TITLE_SIZE)
            draw.text((MARGIN, y), title, font=f_title, fill=(38, 34, 30))
        if subtitle:
            f_sub, font_src = load_font(SUBTITLE_SIZE)
            draw.text((MARGIN, y + (TITLE_SIZE + 18 if title else 0)), subtitle, font=f_sub, fill=(120, 112, 100))
        rule_y = MARGIN + HEADER_H - 20
        draw.line((MARGIN, rule_y, page_w - MARGIN, rule_y), fill=(214, 208, 198), width=2)

        for src, box in zip(chunk, _boxes(layout, page_w, page_h)):
            _draw_photo(page, draw, src, box)

        # 页脚：页码（右对齐）
        f_foot, font_src = load_font(FOOTER_SIZE)
        label = f"{len(pages) + 1} / {max(1, (len(chosen) + capacity - 1) // capacity)}"
        tw = draw.textlength(label, font=f_foot)
        draw.text((page_w - MARGIN - tw, page_h - FOOTER_H + 26), label, font=f_foot, fill=(150, 143, 132))

        pages.append(page)

    if not pages:  # 没有任何可用图：给一张"空版式"，而不是报错崩掉
        page = Image.new("RGB", (page_w, page_h), (255, 255, 255))
        draw = ImageDraw.Draw(page)
        f, font_src = load_font(SUBTITLE_SIZE)
        draw.text((MARGIN, page_h // 2), "没有可用的照片", font=f, fill=(120, 112, 100))
        pages = [page]

    return pages, font_src, min(len(chosen), len(paths))


def export_cards(
    paths: list[str],
    out_dir: str,
    *,
    layout: str = "magazine_3up",
    title: str = "",
    subtitle: str = "",
    formats: list[str] | None = None,
    page_size: str = "a4",
    max_pages: int = 12,
    basename: str = "card",
) -> CardResult:
    """渲染并落盘。`formats` 含 `pdf` 时额外导出多页 PDF。"""
    formats = formats or ["pdf"]
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)

    pages, font_src, used = render_pages(
        paths,
        layout=layout,
        title=title,
        subtitle=subtitle,
        page_size=page_size,
        max_pages=max_pages,
    )

    page_files: list[str] = []
    if any(f in ("jpg", "png") for f in formats):
        for i, page in enumerate(pages):
            dst = str(out / f"{basename}_p{i + 1}.jpg")
            save_jpeg(page, dst, quality=92)
            page_files.append(dst)

    pdf_path: str | None = None
    if "pdf" in formats:
        pdf_path = str(out / f"{basename}.pdf")
        save_pdf(pages, pdf_path, dpi=150)

    capacity = LAYOUT_CAPACITY.get(layout, 3)
    return CardResult(
        pages=page_files,
        pdf=pdf_path,
        page_count=len(pages),
        font_source=font_src,
        layout=layout,
        images_used=used,
        fitted=max(0, len(paths) - used),
    )


__all__ = ["export_cards", "render_pages", "CardResult", "LAYOUT_CAPACITY", "PAGE_SIZES"]
