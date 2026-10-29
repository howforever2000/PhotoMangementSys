"""NPU 可行性探针：用 OpenVINO 直接把现有 ONNX 模型编译到 NPU/GPU/CPU，量单张延迟。

目的：回答「扫描入库能不能吃 NPU」，用实测数字而不是猜。
纪律：只读模型、不改仓库、不改 .venv-vcr（openvino 装在 python/.tmp-ovprobe）。

用法：
    $env:PYTHONPATH="$PWD\\.tmp-ovprobe"
    .\\.venv-vcr\\Scripts\\python.exe bench\\probe_npu_feasibility.py
"""
import os
import sys
import time

import numpy as np
import openvino as ov

MODEL_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "models")

CASES = [
    # (标签, 路径, 输入 shape；None = 不 reshape)
    ("det  yolov8n-det", "yolov8n-det.onnx", [1, 3, 640, 640]),
    ("fd   det_10g(SCRFD)", "det_10g.onnx", [1, 3, 640, 640]),
    ("fr   w600k_r50(ArcFace)", "w600k_r50.onnx", [1, 3, 112, 112]),
    ("ocr  paddleocr-det", "paddleocr-det.onnx", [1, 3, 960, 960]),
    ("clip vision(active tier)", "chinese-clip/clip_vision.onnx", [1, 3, 224, 224]),
    # clip_text 的 attention_mask 带无界动态维，NPU 编译器直接 LLVM abort（会把整个进程带走），
    # 语义上文本塔本来也不是热点，不探。真要跑必须先导出静态 shape 版本。
]
DEVICES = ["NPU", "GPU", "CPU"]


def bench(core, label, path, shape, device, runs=10, warmup=3):
    t0 = time.perf_counter()
    try:
        model = core.read_model(path)
        if shape is not None:
            model.reshape({model.input(0): shape})
        compiled = core.compile_model(model, device)
    except Exception as e:  # noqa: BLE001
        return {"label": label, "device": device, "error": f"{type(e).__name__}: {str(e)[:300]}"}
    load_ms = (time.perf_counter() - t0) * 1000.0

    inp = compiled.input(0)
    try:
        shp = [d.get_length() for d in inp.partial_shape]
    except RuntimeError:
        return {"label": label, "device": device, "load_ms": round(load_ms, 1),
                "error": f"still dynamic: {inp.partial_shape}"}
    if any(not isinstance(d, int) or d <= 0 for d in shp):
        return {"label": label, "device": device, "load_ms": round(load_ms, 1),
                "error": f"still dynamic: {shp}"}
    dtype = np.int64 if "i64" in str(inp.get_element_type()) else np.float32
    x = (np.random.randn(*shp) * 0.1).astype(dtype)
    try:
        for _ in range(warmup):
            compiled(x)
        times = []
        for _ in range(runs):
            t = time.perf_counter()
            compiled(x)
            times.append((time.perf_counter() - t) * 1000.0)
    except Exception as e:  # noqa: BLE001
        return {"label": label, "device": device, "load_ms": round(load_ms, 1),
                "error": f"run {type(e).__name__}: {str(e)[:300]}"}
    times.sort()
    return {
        "label": label, "device": device, "shape": shp, "load_ms": round(load_ms, 1),
        "avg_ms": round(sum(times) / len(times), 2), "min_ms": round(times[0], 2),
        "max_ms": round(times[-1], 2),
    }


def main():
    core = ov.Core()
    print("openvino:", ov.__version__, "| devices:", core.available_devices)
    rows = []
    for label, fname, shape in CASES:
        path = os.path.join(MODEL_DIR, fname)
        if not os.path.isfile(path):
            print(f"SKIP {label}: 缺文件 {path}")
            continue
        for device in DEVICES:
            r = bench(core, label, path, shape, device)
            rows.append(r)
            if "error" in r:
                print(f"{label:26s} {device:4s} ERR  {r['error']}", flush=True)
            else:
                print(f"{label:26s} {device:4s} avg {r['avg_ms']:7.2f} ms  "
                      f"min {r['min_ms']:7.2f}  compile {r['load_ms']:8.1f} ms  {r['shape']}",
                      flush=True)
    print("\n== json ==")
    import json
    print(json.dumps(rows, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    sys.exit(main())
