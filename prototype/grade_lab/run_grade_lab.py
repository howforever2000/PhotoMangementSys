"""原型：把「胶片模拟」与「参考图颜色迁移」跑出前后对比图，供效果评估。

纯 Pillow + numpy，无新依赖。输出到 out/。
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import numpy as np
from PIL import Image, ImageDraw, ImageFont, ImageOps

from colortransfer import histogram_match, reinhard_transfer
from lut import apply_cube, load_cube

HERE = os.path.dirname(os.path.abspath(__file__))
LUTS_DIR = os.path.join(HERE, "luts")
SAMPLES_DIR = os.path.join(HERE, "samples")
OUT_DIR = os.path.join(HERE, "out")

FILM_LUTS = [
    ("Provia 标准", "provia_sRGB.cube"),
    ("Velvia 浓艳", "velvia_sRGB.cube"),
    ("Astia 柔和", "astia_sRGB.cube"),
    ("Classic Chrome 经典铬", "classic chrome_sRGB.cube"),
    ("Classic Neg 经典负片", "classic neg_sRGB.cube"),
    ("Eterna 电影", "eterna_sRGB.cube"),
    ("Nostalgic Neg 怀旧负片", "nostalgic neg_sRGB.cube"),
]


def load_img(path, max_edge=1200):
    im = Image.open(path).convert("RGB")
    w, h = im.size
    m = max(w, h)
    if m > max_edge:
        s = max_edge / m
        im = im.resize((int(w * s), int(h * s)), Image.LANCZOS)
    return im


def _font(size=16):
    for p in (r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\arial.ttf", r"C:\Windows\Fonts\segoeui.ttf"):
        if os.path.exists(p):
            try:
                return ImageFont.truetype(p, size)
            except Exception:
                pass
    return ImageFont.load_default()


def _tile(img, label, tw, th, font, bar=28):
    im = img if isinstance(img, Image.Image) else Image.fromarray(img)
    im = ImageOps.fit(im.convert("RGB"), (tw, th), method=Image.LANCZOS)
    canvas = Image.new("RGB", (tw, th + bar), (255, 255, 255))
    canvas.paste(im, (0, bar))
    d = ImageDraw.Draw(canvas)
    d.text((6, 5), label, fill=(0, 0, 0), font=font)
    return canvas


def montage(items, cols, out_path, tile=(440, 300), gap=8, pad=8):
    tw, th = tile
    font = _font(15)
    bar = 28
    tiles = [_tile(im, lab, tw, th, font, bar) for lab, im in items]
    cell_h = th + bar
    rows = (len(tiles) + cols - 1) // cols
    W = pad * 2 + cols * tw + (cols - 1) * gap
    H = pad * 2 + rows * cell_h + (rows - 1) * gap
    canvas = Image.new("RGB", (W, H), (232, 232, 232))
    for i, t in enumerate(tiles):
        r, c = divmod(i, cols)
        canvas.paste(t, (pad + c * (tw + gap), pad + r * (cell_h + gap)))
    canvas.save(out_path, quality=92)
    return out_path


def run_film_sim():
    base = load_img(os.path.join(SAMPLES_DIR, "night_city.jpg"), max_edge=1200)
    arr = np.array(base)

    # 顺序自检：纯 R/G/B 输入经 LUT 的输出（确认 .cube 轴序未转置）
    probe = np.zeros((1, 3, 3), np.uint8)
    probe[0, 0] = (255, 0, 0)
    probe[0, 1] = (0, 255, 0)
    probe[0, 2] = (0, 0, 255)
    p_lut, p_n = load_cube(os.path.join(LUTS_DIR, FILM_LUTS[0][1]))
    print("LUT size", p_n, "probe(R,G,B) ->", apply_cube(probe, (p_lut, p_n)).tolist())

    items = [("原图 Original", base)]
    for label, fn in FILM_LUTS:
        path = os.path.join(LUTS_DIR, fn)
        if not os.path.exists(path):
            print("skip missing", fn)
            continue
        out = apply_cube(arr, path)
        items.append((label, out))
        Image.fromarray(out).save(os.path.join(OUT_DIR, f"filmsim_{fn.replace('.cube','')}.jpg"), quality=92)
    out = montage(items, 4, os.path.join(OUT_DIR, "filmsim_compare.jpg"))
    print("wrote", out)


def run_color_transfer():
    target = load_img(os.path.join(SAMPLES_DIR, "night_city.jpg"), max_edge=1000)
    refs = [
        ("日落暖调", "login_sunset.jpg"),
        ("青绿冷调", "cover_teal.jpg"),
    ]
    t_arr = np.array(target)
    for tag, ref_name in refs:
        ref_path = os.path.join(SAMPLES_DIR, ref_name)
        if not os.path.exists(ref_path):
            print("skip missing ref", ref_name)
            continue
        ref = load_img(ref_path, max_edge=1000)
        r_arr = np.array(ref)
        rh = reinhard_transfer(t_arr, r_arr)
        hm = histogram_match(t_arr, r_arr)
        Image.fromarray(rh).save(os.path.join(OUT_DIR, f"ct_{tag}_reinhard.jpg"), quality=92)
        Image.fromarray(hm).save(os.path.join(OUT_DIR, f"ct_{tag}_histmatch.jpg"), quality=92)
        items = [
            ("目标图 Target", target),
            (f"参考图 Ref({tag})", ref),
            ("Reinhard(Lab)", rh),
            ("直方图匹配", hm),
        ]
        out = montage(items, 4, os.path.join(OUT_DIR, f"colortransfer_{tag}.jpg"), tile=(420, 300))
        print("wrote", out)


if __name__ == "__main__":
    os.makedirs(OUT_DIR, exist_ok=True)
    run_film_sim()
    run_color_transfer()
