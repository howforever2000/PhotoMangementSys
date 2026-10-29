"""CLIP 图像塔在 NPU 上的数值等价性 + 体积/编译开销探针。

背景：BUG-2026-0910-006 —— fp16 档在非 CPU EP 上曾静默产出错误向量。
NPU 只吃 fp16（内部会把 fp32 权重压成 f16），所以「跑得动」不等于「算得对」。
本脚本对同一输入比 CPU(f32) 与 NPU 输出向量的余弦相似度。

用法：
    $env:PYTHONPATH="$PWD\\.tmp-ovprobe"
    .\\.venv-vcr\\Scripts\\python.exe bench\\probe_npu_clip_equiv.py
"""
import os
import time

import numpy as np
import openvino as ov

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MODEL = os.path.join(HERE, "models", "chinese-clip", "clip_vision.onnx")
CACHE = os.path.join(HERE, "bench", "out", "ov_cache")


def cos(a, b):
    a = a.ravel().astype(np.float64)
    b = b.ravel().astype(np.float64)
    return float(a @ b / (np.linalg.norm(a) * np.linalg.norm(b)))


def main():
    core = ov.Core()
    core.set_property({"CACHE_DIR": CACHE})

    # 固定种子 + 真实 CLIP 输入分布（ImageNet 归一化后的 ~[-2.5, 2.5]）
    rng = np.random.default_rng(0)
    x = rng.standard_normal((1, 3, 224, 224)).astype(np.float32)

    outs = {}
    for dev in ("CPU", "NPU"):
        t0 = time.perf_counter()
        model = core.read_model(MODEL)
        # 必须 reshape 成静态：NPU 编译器遇到未解析的动态维会直接 LLVM abort
        # （不可 catch，整个进程被杀）
        model.reshape({model.input(0): [1, 3, 224, 224]})
        model = core.compile_model(model, dev)
        # NPU 首次 infer 才真正编译，故先 warmup 再看稳态
        for _ in range(3):
            model(x)
        t_infer = (time.perf_counter() - t0) * 1000.0
        y = np.asarray(model(x)[0])
        outs[dev] = y
        print(f"{dev:4s} out shape={y.shape} dtype={y.dtype} "
              f"| 首轮含NPU编译 {t_infer:8.1f} ms | norm={np.linalg.norm(y):.4f}")

    c = cos(outs["CPU"], outs["NPU"])
    d = np.abs(outs["CPU"] - outs["NPU"])
    print(f"\ncosine(CPU_f32, NPU_f16) = {c:.6f}")
    print(f"最大绝对差 = {d.max():.5f} | 平均绝对差 = {d.mean():.6f}")
    print("判定：", "通过（>=0.9999，可直接换 EP）" if c >= 0.9999
          else "不通过 —— 与 BUG-2026-0910-006 同类，NPU 输出与 CPU 不同空间")


if __name__ == "__main__":
    main()
