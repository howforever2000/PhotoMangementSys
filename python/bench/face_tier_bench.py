"""人脸模型档位实测对比：det_500m vs det_10g / w600k_mbf vs w600k_r50

用途（只做测量，不改任何生产代码）：模型换档前拿真实照片量化「精度换速度」的代价，
把数字写进 feature-notes，避免凭直觉选档。

测什么：
  1. 检测：同一张图、同一 letterbox、同一解码路径（FaceService._decode_scrfd）下
     det_500m / det_10g 的耗时与检出人脸数（数量差异 = 误检/漏检的直观信号）。
  2. 识别：同一批已对齐的 112×112 人脸，w600k_mbf / w600k_r50 的单脸耗时。

用法：
  cd python
  python bench/face_tier_bench.py --dir "D:\\YUAN HAO\\Pictures\\2026\\test" --n 12
"""
import argparse
import os
import sys
import time

import numpy as np
import onnxruntime as ort
from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from vcr import config, preprocess  # noqa: E402
from vcr.services.face_service import FaceService  # noqa: E402

MODEL_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "models")


def load(name: str):
    p = os.path.join(MODEL_DIR, name)
    if not os.path.isfile(p):
        return None
    so = ort.SessionOptions()
    so.intra_op_num_threads = config.threads()
    so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
    return ort.InferenceSession(p, sess_options=so, providers=["CPUExecutionProvider"])


def gather_images(d: str, n: int) -> list[str]:
    exts = (".jpg", ".jpeg", ".png", ".webp", ".bmp")
    out: list[str] = []
    for root, _dirs, files in os.walk(d):
        for f in sorted(files):
            if f.lower().endswith(exts):
                out.append(os.path.join(root, f))
                if len(out) >= n:
                    return out
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", required=True)
    ap.add_argument("--n", type=int, default=12)
    args = ap.parse_args()

    dets = {n: load(n) for n in ("det_500m.onnx", "det_10g.onnx")}
    recs = {n: load(n) for n in ("w600k_mbf.onnx", "w600k_r50.onnx")}
    dets = {k: v for k, v in dets.items() if v}
    recs = {k: v for k, v in recs.items() if v}
    if len(dets) < 2 or len(recs) < 2:
        print(f"需要两档模型都在（当前 det={list(dets)} rec={list(recs)}）")
        return 1

    paths = gather_images(args.dir, args.n)
    if not paths:
        print(f"目录无图片: {args.dir}")
        return 1
    print(f"样本 {len(paths)} 张 | threads={config.threads()} | provider=CPU")
    print(f"阈值：FACE_DET_CONF={config.FACE_DET_CONF} FACE_MIN_PIX={config.FACE_MIN_PIX}")

    # 预热（消除首次加载/分配开销）
    warm = Image.new("RGB", (640, 640), (0, 0, 0))
    lbw = preprocess.letterbox(warm)
    for m in dets.values():
        m.run(None, {"input.1": preprocess.face_det_tensor(lbw)[0]})
    warm_face = np.zeros((1, 3, 112, 112), dtype=np.float32)
    for m in recs.values():
        m.run(None, {"input.1": warm_face})

    det_time = {k: [] for k in dets}
    det_faces = {k: 0 for k in dets}
    rec_time = {k: [] for k in recs}
    aligned: list[np.ndarray] = []

    for path in paths:
        try:
            img = Image.open(path)
            img.load()
        except Exception as e:  # noqa: BLE001
            print(f"  跳过（解码失败）{os.path.basename(path)}: {e}")
            continue
        img = img.convert("RGB")
        lb = preprocess.letterbox(img)
        for name, sess in dets.items():
            tensor, scale, px, py = preprocess.face_det_tensor(lb)
            t0 = time.perf_counter()
            outs = sess.run(None, {"input.1": tensor})
            dt = (time.perf_counter() - t0) * 1000.0
            faces = FaceService._decode_scrfd(outs, scale, px, py)
            w, h = lb.size
            kept = []
            for f in faces:
                bw, bh = f.bbox[2] - f.bbox[0], f.bbox[3] - f.bbox[1]
                if min(bw, bh) < config.FACE_MIN_PIX:
                    continue
                if f.bbox[0] < 0 or f.bbox[1] < 0 or f.bbox[2] > w or f.bbox[3] > h:
                    continue
                kept.append(f)
            kept.sort(key=lambda f: f.score, reverse=True)
            kept = FaceService._nms_faces(kept)
            det_time[name].append(dt)
            det_faces[name] += len(kept)
            if name == "det_10g.onnx":
                for f in kept[:6]:
                    aligned.append(preprocess.face_align(img, f.kps))
            tag = "500m" if "500m" in name else "10g "
            print(f"  {os.path.basename(path)[:34]:36s} {tag}: {dt:6.1f}ms\t{len(kept)} 脸")

    print("\n=== 检测（同一批图，含解码+后处理）===")
    for k, ts in det_time.items():
        a = np.array(ts)
        print(f"  {k:16s} 均 {a.mean():7.2f}ms 中位 {np.median(a):7.2f}ms 最快 {a.min():6.2f}ms | 检出 {det_faces[k]} 张脸")

    print(f"\n=== 识别（{len(aligned)} 张对齐脸，单脸前向）===")
    for name, sess in recs.items():
        for t in aligned:
            t0 = time.perf_counter()
            sess.run(None, {"input.1": t})
            rec_time[name].append((time.perf_counter() - t0) * 1000.0)
        a = np.array(rec_time[name])
        print(f"  {name:16s} 均 {a.mean():6.2f}ms 中位 {np.median(a):6.2f}ms 最快 {a.min():6.2f}ms（n={len(a)}）")

    dm = np.mean(det_time["det_10g.onnx"]) / max(np.mean(det_time["det_500m.onnx"]), 1e-6)
    rm = np.mean(rec_time["w600k_r50.onnx"]) / max(np.mean(rec_time["w600k_mbf.onnx"]), 1e-6)
    print(f"\n换档代价：检测 {dm:.2f}x，识别 {rm:.2f}x（每张照片人脸通道成本 ≈ 检测 + 脸数×识别）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
