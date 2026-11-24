"""把四列对比再排成 2×2 网格，便于在聊天窗口里看清细节。"""
from __future__ import annotations

import os
from PIL import Image, ImageDraw, ImageFont

ROOT = r"C:\Users\Admin\Documents\Projects\PhotoMangementSys"
AB = os.path.join(ROOT, "lab", "natural_grade_ab")
OUT = os.path.join(AB, "out")
CMP = os.path.join(AB, "compare")

PANELS = [
    ("原图", "原图"),
    ("现行natural85", "现行 自然 0.85"),
    ("v2保守", "新方案 · 保守"),
    ("v2标准", "新方案 · 标准"),
]
W = 900
GAP = 12
BAR = 46


def fit(im: Image.Image, width: int) -> Image.Image:
    w, h = im.size
    return im.resize((width, max(1, round(width * h / w))), Image.LANCZOS)


def grid(name: str, out_path: str) -> None:
    panels = []
    for folder, label in PANELS:
        im = Image.open(os.path.join(OUT, folder, name)).convert("RGB")
        panels.append((fit(im, W), label))

    h = max(p.size[1] for p, _ in panels)
    canvas = Image.new("RGB", (2 * W + GAP, 2 * (BAR + h) + GAP), (24, 24, 26))
    draw = ImageDraw.Draw(canvas)
    try:
        font = ImageFont.truetype(r"C:\Windows\Fonts\msyh.ttc", 28)
    except OSError:
        font = ImageFont.load_default()
    for i, (p, label) in enumerate(panels):
        col, row = i % 2, i // 2
        x = col * (W + GAP)
        y = row * (BAR + h + GAP)
        draw.text((x + 16, y + 10), label, fill=(235, 235, 235), font=font)
        canvas.paste(p, (x, y + BAR))
    canvas.save(out_path, quality=93)


def main() -> None:
    files = [f for f in os.listdir(os.path.join(OUT, "原图")) if f.lower().endswith(".jpg")]
    for name in sorted(files):
        grid(name, os.path.join(CMP, f"{os.path.splitext(name)[0]}_2x2.jpg"))
        print("grid", name)


if __name__ == "__main__":
    main()
