"""全库缩略图 → Chinese-CLIP 向量（端到端质量验证用，DML b=32）

产出 python/bench/out/：
  embeddings.npy  (N,512) fp32，已 L2 归一化
  paths.json      与矩阵行对应的缩略图路径
"""
import os
import sys
import time
import json

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from bench_clip import list_thumbs, preprocess  # noqa: E402

MODEL_DIR = os.path.join(HERE, "..", "models", "chinese-clip-vit-b-16")
OUT = os.path.join(HERE, "out")
BATCH = 32

import onnxruntime as ort  # noqa: E402


def main():
    os.makedirs(OUT, exist_ok=True)
    thumbs = list_thumbs()
    print(f"全库 {len(thumbs)} 张，DML b={BATCH}")
    sess = ort.InferenceSession(
        os.path.join(MODEL_DIR, "clip_vision_b16.onnx"),
        ort.SessionOptions(), providers=["DmlExecutionProvider"])
    inp = sess.get_inputs()[0].name

    vecs, paths, failed = [], [], []
    t0 = time.perf_counter()
    for i in range(0, len(thumbs), BATCH):
        chunk = thumbs[i:i + BATCH]
        try:
            pixels = [preprocess(fp) for fp in chunk]
            out = sess.run(None, {inp: np.stack(pixels)})[0]
            vecs.append(out.astype(np.float32))
            paths.extend(chunk)
        except Exception as e:
            failed.extend(chunk)
            print("  跳过失败批次:", str(e)[:80])
        if (i // BATCH) % 20 == 0:
            done = i + len(chunk)
            rate = done / (time.perf_counter() - t0)
            print(f"  {done}/{len(thumbs)}  {rate:.1f} 张/s  ETA {(len(thumbs)-done)/rate/60:.1f} min")
    emb = np.vstack(vecs)
    emb /= (np.linalg.norm(emb, axis=1, keepdims=True) + 1e-9)
    np.save(os.path.join(OUT, "embeddings.npy"), emb)
    json.dump(paths, open(os.path.join(OUT, "paths.json"), "w"))
    dt = time.perf_counter() - t0
    print(f"完成: {emb.shape}, 总耗时 {dt/60:.1f} min（{len(thumbs)/dt:.1f} 张/s），失败 {len(failed)} 张")


if __name__ == "__main__":
    main()
