"""100% 细节对比：检查天空渐变/建筑边缘是否有色带、光晕、过度锐化。"""
from __future__ import annotations

import os
from PIL import Image, ImageDraw, ImageFont

ROOT = r"C:\Users\Admin\Documents\Projects\PhotoMangementSys"
OUT = os.path.join(ROOT, "lab", "natural_grade_ab", "out")
CMP = os.path.join(ROOT, "lab", "natural_grade_ab", "compare")

# (文件, 左上角 x, y, 边长)
CROPS = {
    "IMG_20261121_122632.jpg": (900, 250, 760),     # 天空渐变
    "IMG_20261121_154552.jpg": (600, 700, 760),     # 楼体边缘/阴影
    "IMG_20261120_175422.jpg": (700, 600, 760),     # 晚霞渐变
}
PANELS = [("原图", "原图"), ("现行natural85", "现行 自然 0.85"), ("v2标准", "新方案 · 标准")]
W = 760
BAR = 44
GAP = 10


def main() -> None:
    try:
        font = ImageFont.truetype(r"C:\Windows\Fonts\msyh.ttc", 26)
    except OSError:
        font = ImageFont.load_default()
    for name, (x, y, s) in CROPS.items():
        panels = []
        for folder, label in PANELS:
            im = Image.open(os.path.join(OUT, folder, name)).convert("RGB")
            crop = im.crop((x, y, x + s, y + s))
            panels.append((crop, label))
        canvas = Image.new("RGB", (len(panels) * W + (len(panels) - 1) * GAP, BAR + s), (24, 24, 26))
        draw = ImageDraw.Draw(canvas)
        for i, (crop, label) in enumerate(panels):
            px = i * (W + GAP)
            draw.text((px + 14, 8), label, fill=(235, 235, 235), font=font)
            canvas.paste(crop, (px, BAR))
        out = os.path.join(CMP, f"{os.path.splitext(name)[0]}_crop100.jpg")
        canvas.save(out, quality=95)
        print(out)


if __name__ == "__main__":
    main()
