"""CLIP 图像塔：fp16 档 vs fp32 档 在 CPU / iGPU / NPU 上的延迟 + 数值等价性。

回答「fp32/fp16 都能上 NPU 吗」。纪律同 BUG-2026-0910-006：
跑得动 != 算得对，必须比余弦。

用法：
    $env:PYTHONPATH="$PWD\\.tmp-ovprobe"
    .\\.venv-vcr\\Scripts\\python.exe bench\\probe_npu_clip_tiers.py
"""
import os
import time

import numpy as np
import openvino as ov

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CACHE = os.path.join(HERE, "bench", "out", "ov_cache")
TIERS = [
    ("fp16", os.path.join(HERE, "models", "chinese-clip", "clip_vision.onnx")),
    ("fp32", os.path.join(HERE, "models", "chinese-clip-fp32", "clip_vision.onnx")),
]
DEVICES = ["CPU", "GPU", "NPU"]


def cos(a, b):
    a, b = a.ravel().astype(np.float64), b.ravel().astype(np.float64)
    return float(a @ b / (np.linalg.norm(a) * np.linalg.norm(b)))


def main():
    core = ov.Core()
    core.set_property({"CACHE_DIR": CACHE})
    x = np.random.default_rng(0).standard_normal((1, 3, 224, 224)).astype(np.float32)

    for tier, path in TIERS:
        if not os.path.isfile(path):
            print(f"[{tier}] 缺文件 {path}")
            continue
        size_mb = os.path.getsize(path) / 1024 / 1024
        print(f"\n=== {tier} 档  {size_mb:.1f} MB  {os.path.basename(os.path.dirname(path))} ===")
        ref, outs = None, {}
        for dev in DEVICES:
            try:
                t0 = time.perf_counter()
                m = core.read_model(path)
                m.reshape({m.input(0): [1, 3, 224, 224]})
                cm = core.compile_model(m, dev)
                cold = (time.perf_counter() - t0) * 1000.0   # NPU 真正的编译在首次 infer
                for _ in range(3):
                    cm(x)
                cold_total = (time.perf_counter() - t0) * 1000.0
                ts = []
                for _ in range(10):
                    t = time.perf_counter()
                    cm(x)
                    ts.append((time.perf_counter() - t) * 1000.0)
                ts.sort()
                y = np.asarray(cm(x)[0])
                outs[dev] = y
                if dev == "CPU":
                    ref = y
                sim = cos(ref, y) if ref is not None else float("nan")
                print(f"  {dev:4s} avg {sum(ts)/len(ts):7.2f} ms | min {ts[0]:7.2f} | "
                      f"首轮(含编译) {cold_total:8.0f} ms | cos(vs CPU) {sim:.6f}")
            except Exception as e:  # noqa: BLE001
                print(f"  {dev:4s} ERR {type(e).__name__}: {str(e)[:200]}")


if __name__ == "__main__":
    main()
