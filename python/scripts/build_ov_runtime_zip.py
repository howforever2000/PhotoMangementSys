"""构建「OpenVINO 裁剪运行时」组件包（FEAT-133）。

产出：`python/dist/openvino-npu-win-x64.zip`（约 13~15MB，装后 40.7MB）

上传：作为 GitHub Release 资产上传，文件名**必须**逐字保持
`openvino-npu-win-x64.zip` —— `src-tauri/src/model_dl.rs` 的
`OPENVINO_RUNTIME[0].abs_url` 用的是
`.../releases/latest/download/openvino-npu-win-x64.zip`，
`vcr/ovrt.py` 的 `ZIP_NAME` 也必须是同一个名字。

## 为什么裁这些

完整 openvino 装后 244.8MB，实测（2026.4.1）逐项：

    openvino_intel_npu_compiler.dll   95.7MB  编图用 —— 每次启动现编现用，运行期不需要
    openvino_intel_cpu_plugin.dll     44.2MB  我们不用 OV 跑 CPU（语义塔只在 NPU/GPU 时才走 OV）
    openvino_intel_gpu_plugin.dll     33.1MB  同（GPU 路径留作降级候选，但 Intel 上 NPU 才是收益点）
    openvino.dll                      15.7MB  必需
    openvino_intel_npu_plugin.dll      7.3MB  必需
    openvino_onnx_frontend.dll         5.2MB  必需（读 .onnx）
    openvino_pytorch/paddle/tf/gguf    ~13MB   不需要
    cache.json                         8.4MB   不需要
    *.lib / *.pdb                      8MB+   不需要（链接/调试符号）

裁剪后 `python/bench/probe_npu_blob_size.py` 实测：
`import_model()` 345ms、稳态 22.34ms、与完整版输出 **cos = 1.00000000**。

> 保留 GPU 插件时需要回来改 DROP 列表（本脚本目前把它列进 DROP，理由是
> 「Intel 机器上 NPU 是收益点，GPU 路径交给 ONNX Runtime 的 DirectML 更省一份运行时」）。
> 若将来要让 OV 接管 iGPU 降级，把 `openvino_intel_gpu_plugin.dll` 从 DROP 移出即可。

用法：
    python python/scripts/build_ov_runtime_zip.py
    python python/scripts/build_ov_runtime_zip.py --keep     # 保留临时目录以便核对
"""
from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile

# 与 vcr/ovrt.py 的 ZIP_NAME 必须逐字一致；model_dl.rs 的 local 同理
ZIP_NAME = "openvino-npu-win-x64.zip"
# 实测通过的版本；升版前必须重跑 bench/probe_npu_clip_tiers.py（数值等价 + 延迟）
OPENVINO_VERSION = "2026.4.1"

HERE = os.path.dirname(os.path.abspath(__file__))
PY_DIR = os.path.dirname(HERE)
DIST_DIR = os.path.join(PY_DIR, "dist")

DROP_FILES = {
    # 编译器：只在「ONNX → NPU blob」时需要，我们启动时现编（有 CACHE_DIR 缓存）
    "openvino_intel_npu_compiler.dll",
    # 插件：本组件只服务 NPU；CPU/GPU 路径由 ONNX Runtime 承担
    "openvino_intel_cpu_plugin.dll",
    "openvino_intel_gpu_plugin.dll",
    # 非 onnx 前端
    "openvino_tensorflow_frontend.dll",
    "openvino_pytorch_frontend.dll",
    "openvino_paddle_frontend.dll",
    "openvino_gguf_frontend.dll",
    "openvino_ir_frontend.dll",
    # 体积大户杂项
    "cache.json",
}
DROP_SUFFIX = (".lib", ".pdb", ".exp", ".chm")


def dir_size_mb(path: str) -> float:
    total = 0
    for root, _dirs, files in os.walk(path):
        for fn in files:
            total += os.path.getsize(os.path.join(root, fn))
    return total / 1024 / 1024


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--keep", action="store_true", help="保留临时目录")
    ap.add_argument("--version", default=OPENVINO_VERSION)
    args = ap.parse_args()

    work = tempfile.mkdtemp(prefix="ovrt-build-")
    target = os.path.join(work, "site")
    try:
        print(f"[1/4] pip install openvino=={args.version} → {target}")
        r = subprocess.run(
            [sys.executable, "-m", "pip", "install", "--target", target,
             f"openvino=={args.version}", "--no-warn-script-location",
             "--disable-pip-version-check", "-q"],
            capture_output=True, text=True,
        )
        if r.returncode != 0:
            print(r.stdout[-2000:])
            print(r.stderr[-2000:])
            return 1

        pkg = os.path.join(target, "openvino")
        if not os.path.isdir(pkg):
            print(f"❌ 安装后未找到 {pkg}")
            return 1
        print(f"      完整包体 {dir_size_mb(pkg):.1f} MB")

        print("[2/4] 裁剪")
        removed = 0.0
        for root, _dirs, files in os.walk(pkg):
            for fn in files:
                p = os.path.join(root, fn)
                if fn.lower() in DROP_FILES or fn.lower().endswith(DROP_SUFFIX):
                    removed += os.path.getsize(p) / 1024 / 1024
                    os.remove(p)
        # 剔掉打包器/构建期才用的东西
        for extra in ("__pycache__",):
            shutil.rmtree(os.path.join(pkg, extra), ignore_errors=True)
        trimmed = dir_size_mb(pkg)
        print(f"      裁剪后 {trimmed:.1f} MB（删掉 {removed:.1f} MB）")

        print("[3/4] 自检：裁剪版能否只靠自身 import 并列出设备")
        env = dict(os.environ, PYTHONPATH=target)
        code = (
            "import openvino as ov;"
            "core=ov.Core();"
            "print('  openvino', ov.__version__, '| devices:', core.available_devices)"
        )
        r = subprocess.run([sys.executable, "-c", code], env=env, capture_output=True, text=True)
        print(r.stdout.strip() or r.stderr.strip()[-800:])
        if r.returncode != 0:
            print("❌ 裁剪版 import 失败，DROP 列表裁多了")
            return 1

        print("[4/4] 打包")
        os.makedirs(DIST_DIR, exist_ok=True)
        out = os.path.join(DIST_DIR, ZIP_NAME)
        if os.path.isfile(out):
            os.remove(out)
        with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
            for root, _dirs, files in os.walk(pkg):
                for fn in files:
                    full = os.path.join(root, fn)
                    arc = os.path.join("openvino", os.path.relpath(full, pkg))
                    zf.write(full, arc.replace("\\", "/"))
        size_mb = os.path.getsize(out) / 1024 / 1024
        sha = hashlib.sha256(open(out, "rb").read()).hexdigest()
        print(f"      {out}\n      {size_mb:.1f} MB · sha256={sha[:16]}…")
        print(
            "\n下一步：把该 zip 作为 GitHub Release 资产上传，文件名保持 "
            f"{ZIP_NAME}\n"
            "         （model_dl 用 releases/latest/download/ 抓它，不必改 URL）"
        )
        return 0
    finally:
        if args.keep:
            print(f"临时目录保留: {work}")
        else:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
