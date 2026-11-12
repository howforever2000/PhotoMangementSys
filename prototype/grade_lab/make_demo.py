"""用生产模块生成三功能对比图（供目视验收）。"""
import os
import sys
import types

sys.modules.setdefault("requests", types.ModuleType("requests"))
sys.path.insert(0, os.path.abspath("python"))

import numpy as np  # noqa: E402
from PIL import Image, ImageDraw  # noqa: E402

from ai_studio import colortransfer, localmodels, lutcube  # noqa: E402

OUT = "prototype/grade_lab/out/demo"
os.makedirs(OUT, exist_ok=True)
S = "prototype/grade_lab/samples"


def tile(im, w, label):
    im = im.convert("RGB")
    h = max(1, int(im.height * w / im.width))
    im = im.resize((w, h), Image.LANCZOS)
    band = 26
    canvas = Image.new("RGB", (w, h + band), (250, 250, 250))
    canvas.paste(im, (0, band))
    ImageDraw.Draw(canvas).text((6, 6), label, fill=(20, 20, 20))
    return canvas


def row(items, out_path, w=540):
    tiles = [tile(im, w, lb) for im, lb in items]
    gap = 12
    W = sum(t.width for t in tiles) + gap * (len(tiles) - 1)
    H = max(t.height for t in tiles)
    canvas = Image.new("RGB", (W, H), (255, 255, 255))
    x = 0
    for t in tiles:
        canvas.paste(t, (x, 0))
        x += t.width + gap
    canvas.save(out_path, quality=93)
    print("saved", out_path, canvas.size)


src = Image.open(f"{S}/night_city.jpg").convert("RGB")
arr = np.asarray(src)
ref = np.asarray(Image.open(f"{S}/login_sunset.jpg").convert("RGB"))

# 1) 胶片
film_velvia = Image.fromarray(lutcube.apply_cube(arr, "python/ai_studio/luts/velvia.cube", 1.0))
film_chrome = Image.fromarray(lutcube.apply_cube(arr, "python/ai_studio/luts/classic_chrome.cube", 1.0))
row([(src, "Original"), (film_velvia, "Film: Velvia"), (film_chrome, "Film: Classic Chrome")],
    f"{OUT}/film_compare.jpg")

# 2) 参考图调色
ct_rein = Image.fromarray(colortransfer.reinhard_transfer(arr, ref, 0.85))
ct_hist = Image.fromarray(colortransfer.histogram_match(arr, ref, 0.7))
row([(src, "Original"), (ct_rein, "Transfer: Reinhard"), (ct_hist, "Transfer: Histogram")],
    f"{OUT}/ct_compare.jpg")

# 3) 超分（中心裁剪 480x300，4x）
w, h = src.size
crop = src.crop((w // 2 - 240, h // 2 - 150, w // 2 + 240, h // 2 + 150))
up = localmodels.run_esrgan(crop, tile=256)
bic = crop.resize((crop.width * 4, crop.height * 4), Image.LANCZOS)
row([(bic, "Bicubic x4"), (up, "Real-ESRGAN x4")], f"{OUT}/sr_compare.jpg")

# 4) 人脸修复（给紧裁人脸补边模拟真实照片上下文，2x）
face = Image.open(f"{S}/face_adele.png").convert("RGB")
pad = Image.new("RGB", (int(face.width * 1.8), int(face.height * 1.8)), (120, 120, 120))
pad.paste(face, ((pad.width - face.width) // 2, (pad.height - face.height) // 2))
pad2 = pad.resize((pad.width * 2, pad.height * 2), Image.LANCZOS)
faces = localmodels.detect_faces(pad)
print("faces detected", len(faces), [round(f["score"], 3) for f in faces])
rest = localmodels.restore_faces(pad2, faces, ratio=2) if faces else pad2
row([(pad2, "Before (2x, blur)"), (rest, "GFPGAN restored")], f"{OUT}/face_compare.jpg")
