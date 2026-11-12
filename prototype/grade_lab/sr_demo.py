"""原型：用 Real-ESRGAN ONNX（facefusion 导出的 real_esrgan_x4）做真实场景超分对比。
输出：原图(参考) / Bicubic×4 放大 / Real-ESRGAN×4 放大，另加 100% 细节裁切对比。
"""
import os
import sys
import time

import numpy as np
import onnxruntime as ort
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from run_grade_lab import _font, montage, OUT_DIR, SAMPLES_DIR  # noqa: E402

MODEL = os.path.join(HERE, "models", "real_esrgan_x4.onnx")


def load_rgb(path, max_edge=800):
    im = Image.open(path).convert("RGB")
    w, h = im.size
    m = max(w, h)
    if m > max_edge:
        s = max_edge / m
        im = im.resize((int(w * s), int(h * s)), Image.LANCZOS)
    return im


def run_esrgan(arr_u8, sess):
    inp = sess.get_inputs()[0]
    x = (arr_u8.astype(np.float32) / 255.0).transpose(2, 0, 1)[None]
    out = sess.run(None, {inp.name: x})[0]
    y = np.clip(out[0].transpose(1, 2, 0), 0.0, 1.0)
    return (y * 255.0 + 0.5).astype(np.uint8)


def main():
    base = load_rgb(os.path.join(SAMPLES_DIR, "night_city.jpg"), max_edge=800)
    w, h = base.size
    lr = base.resize((max(1, w // 4), max(1, h // 4)), Image.LANCZOS)
    bic = lr.resize((w, h), Image.BICUBIC)

    if not os.path.exists(MODEL):
        print("MODEL MISSING:", MODEL)
        return
    t0 = time.time()
    sess = ort.InferenceSession(MODEL, providers=["CPUExecutionProvider"])
    i = sess.get_inputs()[0]
    print("model input:", i.name, i.shape, i.type, "| load", round(time.time() - t0, 2), "s")
    t1 = time.time()
    sr = run_esrgan(np.array(lr), sess)
    print("esrgan out:", sr.shape, "| infer", round(time.time() - t1, 2), "s")
    sr_img = Image.fromarray(sr)
    if sr_img.size != (w, h):
        sr_img = sr_img.resize((w, h), Image.LANCZOS)

    items = [
        ("原图 Ground truth (参考)", base),
        ("低清 LR ×4 Bicubic", bic),
        ("Real-ESRGAN ×4", sr_img),
    ]
    out1 = montage(items, 3, os.path.join(OUT_DIR, "sr_compare.jpg"), tile=(560, 360))
    print("wrote", out1)

    # 100% 细节裁切：中心区域，2x 最近邻放大看锐度
    cw, ch = 240, 150
    cx, cy = w // 2 - cw // 2, h // 2 - ch // 2
    box = (cx, cy, cx + cw, cy + ch)

    def crop2x(im):
        return im.crop(box).resize((cw * 2, ch * 2), Image.NEAREST)

    detail = [
        ("原图细节", crop2x(base)),
        ("Bicubic 细节", crop2x(bic)),
        ("Real-ESRGAN 细节", crop2x(sr_img)),
    ]
    out2 = montage(detail, 3, os.path.join(OUT_DIR, "sr_detail.jpg"), tile=(480, 300))
    print("wrote", out2)

    Image.fromarray(sr).save(os.path.join(OUT_DIR, "sr_realesrgan_x4_full.jpg"), quality=92)


if __name__ == "__main__":
    main()
