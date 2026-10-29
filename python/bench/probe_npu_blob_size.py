"""NPU「不带 OpenVINO 编译器」可行性探针：离线编 blob + 裁剪运行时。

问题：openvino 装完 297MB，其中 openvino_intel_npu_compiler.dll 独占 95.7MB。
      「编译」和「执行」能不能拆开 —— 开发期编好 blob，运行期只带最小运行时？

做法：
  1) 完整 openvino：clip_vision 静态化 → 编译到 NPU → export_model 导出 blob
  2) 构造裁剪版运行时目录（删编译器/CPU插件/GPU插件/多余前端）
  3) 子进程只用裁剪版：import_model(blob) → 推理 → 与完整版比余弦 + 量体积

用法：
    $env:PYTHONPATH="$PWD\\.tmp-ovprobe"
    .\\.venv-vcr\\Scripts\\python.exe bench\\probe_npu_blob_size.py
"""
import os
import shutil
import subprocess
import sys

import numpy as np
import openvino as ov

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MODEL = os.path.join(HERE, "models", "chinese-clip", "clip_vision.onnx")
SRC_PKG = os.path.join(HERE, ".tmp-ovprobe", "openvino")
TRIM_ROOT = os.path.join(HERE, ".tmp-ovtrim")
TRIM_PKG = os.path.join(TRIM_ROOT, "openvino")
BLOB = os.path.join(HERE, "bench", "out", "clip_vision_npu.blob")

# 执行期不需要的东西：编译器（95.7MB 大头）、CPU/GPU 插件、多余前端、符号与 cache
DROP = [
    "openvino_intel_npu_compiler.dll",
    "openvino_intel_cpu_plugin.dll",
    "openvino_intel_gpu_plugin.dll",
    "openvino_onnx_frontend.dll",
    "openvino_tensorflow_frontend.dll",
    "openvino_pytorch_frontend.dll",
    "openvino_paddle_frontend.dll",
    "openvino_gguf_frontend.dll",
    "cache.json",
]
DROP_EXT = (".lib", ".pdb", ".exp")


def dir_mb(path):
    return sum(f.stat().st_size for f in __import__("pathlib").Path(path).rglob("*") if f.is_file()) / 1024 / 1024


def main():
    os.makedirs(os.path.dirname(BLOB), exist_ok=True)
    x = np.random.default_rng(0).standard_normal((1, 3, 224, 224)).astype(np.float32)

    # ---------- 1) 完整版：编译 + 导出 blob ----------
    print(f"[1] 完整 openvino {ov.__version__} | 包体 {dir_mb(SRC_PKG):.1f} MB")
    core = ov.Core()
    model = core.read_model(MODEL)
    model.reshape({model.input(0): [1, 3, 224, 224]})
    cm = core.compile_model(model, "NPU")
    y_full = np.asarray(cm(x)[0])
    import io

    buf = io.BytesIO()
    cm.export_model(buf)
    blob_bytes = buf.getvalue()
    with open(BLOB, "wb") as f:
        f.write(blob_bytes)
    blob_mb = os.path.getsize(BLOB) / 1024 / 1024
    print(f"    blob 导出成功：{blob_mb:.1f} MB -> {os.path.basename(BLOB)}")
    del cm, model, core

    # ---------- 2) 构造裁剪运行时 ----------
    if os.path.isdir(TRIM_ROOT):
        shutil.rmtree(TRIM_ROOT)
    shutil.copytree(SRC_PKG, TRIM_PKG)
    removed = 0.0
    for root, _, files in os.walk(TRIM_PKG):
        for fn in files:
            p = os.path.join(root, fn)
            if fn in DROP or fn.lower().endswith(DROP_EXT):
                removed += os.path.getsize(p) / 1024 / 1024
                os.remove(p)
    print(f"[2] 裁剪后包体 {dir_mb(TRIM_PKG):.1f} MB（删掉 {removed:.1f} MB）")

    # ---------- 3) 子进程：只用裁剪版 import blob 并推理 ----------
    code = f'''
import sys, time
import numpy as np, openvino as ov
core = ov.Core()
print("   devices(裁剪版可见):", core.available_devices)
t0 = time.perf_counter()
cm = core.import_model(open(r"{BLOB}", "rb").read(), "NPU")
t_imp = (time.perf_counter() - t0) * 1000
x = np.random.default_rng(0).standard_normal((1, 3, 224, 224)).astype(np.float32)
for _ in range(3): cm(x)
ts = []
for _ in range(10):
    t = time.perf_counter(); y = np.asarray(cm(x)[0]); ts.append((time.perf_counter()-t)*1000)
np.save(r"{os.path.join(HERE, "bench", "out", "npu_trim_y.npy")}", y)
print(f"   import+首次infer {{t_imp:.0f}} ms | 稳态 {{sum(ts)/len(ts):.2f}} ms")
'''
    env = dict(os.environ, PYTHONPATH=TRIM_ROOT)
    print("[3] 子进程（裁剪版运行时）:")
    r = subprocess.run([sys.executable, "-c", code], env=env,
                       capture_output=True, text=True)
    print(r.stdout.strip() or "(无输出)")
    if r.returncode != 0:
        tail = (r.stderr or "").strip().splitlines()[-6:]
        print("   [FAIL] 子进程非零退出：")
        for line in tail:
            print("     ", line)
        print("\n结论：编译器 DLL 在执行期也必需 → 裁剪路线不成立，需另选方案")
        return 1

    y_trim = np.load(os.path.join(HERE, "bench", "out", "npu_trim_y.npy"))
    a, b = y_full.ravel().astype(np.float64), y_trim.ravel().astype(np.float64)
    cos = float(a @ b / (np.linalg.norm(a) * np.linalg.norm(b)))
    print(f"\n结论：[OK] 裁剪运行时可用 —— cos(完整, 裁剪) = {cos:.8f}")
    print(f"     运行时体积：{dir_mb(SRC_PKG):.1f} MB → {dir_mb(TRIM_PKG):.1f} MB"
          f"（+ 模型 blob {blob_mb:.1f} MB）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
