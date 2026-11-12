"""原型：GFPGAN 1.4 ONNX 人脸修复对比（用 GFPGAN 官方对齐样例脸做「退化→修复」）。
输出：原图(参考) / 退化低清 / GFPGAN 修复。
"""
import os
import sys
import time
import urllib.request

import numpy as np
import onnxruntime as ort
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from run_grade_lab import montage, OUT_DIR, SAMPLES_DIR  # noqa: E402

MODEL = os.path.join(HERE, "models", "gfpgan_1.4.onnx")
FACE_URL = "https://raw.githubusercontent.com/TencentARC/GFPGAN/master/inputs/cropped_faces/Adele_crop.png"
FACE_PATH = os.path.join(SAMPLES_DIR, "face_adele.png")


def ensure_face():
    if os.path.exists(FACE_PATH) and os.path.getsize(FACE_PATH) > 1000:
        return
    for i in range(40):
        try:
            req = urllib.request.Request(FACE_URL, headers={"User-Agent": "dl"})
            with urllib.request.urlopen(req, timeout=40) as r, open(FACE_PATH, "wb") as f:
                f.write(r.read())
            print("face downloaded", os.path.getsize(FACE_PATH))
            return
        except Exception as e:
            print("face retry", i, type(e).__name__, str(e)[:90])
            time.sleep(2)


def norm(im512):
    x = np.asarray(im512, np.float32) / 255.0
    x = (x - 0.5) / 0.5
    return x.transpose(2, 0, 1)[None]


def denorm(out):
    y = out[0].transpose(1, 2, 0)
    y = y * 0.5 + 0.5
    return (np.clip(y, 0, 1) * 255 + 0.5).astype(np.uint8)


def main():
    ensure_face()
    if not os.path.exists(MODEL):
        print("MODEL MISSING", MODEL)
        return
    if not os.path.exists(FACE_PATH):
        print("FACE MISSING", FACE_PATH)
        return

    gt = Image.open(FACE_PATH).convert("RGB").resize((512, 512), Image.LANCZOS)
    # 退化：缩到 64 + JPEG q25 + 上采样回 512
    small = gt.resize((64, 64), Image.LANCZOS)
    tmp = os.path.join(OUT_DIR, "_face_degrade.jpg")
    small.save(tmp, quality=25)
    degraded = Image.open(tmp).convert("RGB").resize((512, 512), Image.BICUBIC)

    sess = ort.InferenceSession(MODEL, providers=["CPUExecutionProvider"])
    i = sess.get_inputs()[0]
    print("gfpgan input:", i.name, i.shape, i.type)
    t = time.time()
    out = sess.run(None, {i.name: norm(degraded)})[0]
    print("gfpgan out:", out.shape, "| infer", round(time.time() - t, 2), "s")
    restored = Image.fromarray(denorm(out))

    items = [
        ("原图 (参考)", gt),
        ("退化 Low-quality", degraded),
        ("GFPGAN 修复", restored),
    ]
    p = montage(items, 3, os.path.join(OUT_DIR, "face_compare.jpg"), tile=(400, 400))
    print("wrote", p)
    restored.save(os.path.join(OUT_DIR, "face_restored_full.jpg"), quality=95)


if __name__ == "__main__":
    main()
