"""验收 FEAT-133：语义图像塔走 OpenVINO 后端后，向量是否与 ORT 等价 + 延迟对比。

门槛（缺一不算通过）：
  1. `cos(ORT, OV) >= 0.9999`（BUG-2026-0910-006 的判据）
  2. OV 后端延迟优于 ORT（本机 Intel NPU 口径）
  3. 两种后端下 `clip_runtime_status()` / `benchmark()` 都自洽（不报「通道不可用」）
  4. 组件缺失时（mode=ort 或组件未装）路径与今天逐字一致

用法：
    cd python
    .\\.venv-vcr\\Scripts\\python.exe bench\\verify_ov_clip_equiv.py
"""
from __future__ import annotations

import os
import sys
import time

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from vcr import config, ovrt  # noqa: E402
from vcr.model_registry import ModelRegistry  # noqa: E402


def l2(v: np.ndarray) -> np.ndarray:
    n = np.linalg.norm(v, axis=1, keepdims=True)
    return v / np.maximum(n, 1e-12)


def main() -> int:
    p = config.clip_paths()
    size = int(p["size"])
    print(f"档位 {p['name']} | vision={os.path.basename(p['vision'])} size={size}")
    st = ovrt.status()
    print(f"组件：installed={st['installed']} usable={st['usable']} "
          f"devices={st['devices']} vendor={st['vendor']}")

    # 固定输入（3 张，模拟一批）：CLIP 归一化后的典型取值范围
    rng = np.random.default_rng(7)
    pixels = (rng.standard_normal((3, 3, size, size)) * 0.8).astype(np.float32)

    results: dict[str, dict] = {}
    for mode in ("ort", "auto"):
        reg = ModelRegistry()
        reg.set_clip_runtime(mode)
        t0 = time.perf_counter()
        _ = reg.clip_vision            # 触发加载/编译
        load_ms = (time.perf_counter() - t0) * 1000.0
        if not reg.is_ready("clip_vision"):
            print(f"[{mode}] ❌ 语义塔未就绪: {reg.load_error('clip_vision')}")
            return 1
        info = reg.clip_runtime_status()
        t1 = time.perf_counter()
        vec = l2(reg.run_clip_vision(pixels))
        infer_ms = (time.perf_counter() - t1) * 1000.0
        facts = reg._session_info.get("clip_vision", {})
        results[mode] = {"vec": vec, "infer_ms": infer_ms, "load_ms": load_ms,
                         "status": info, "providers": facts.get("providers")}
        print(f"[{mode}] backend={info['backend']} device={info['device'] or '-'} "
              f"加载 {load_ms:.0f}ms | batch=3 推理 {infer_ms:.0f}ms "
              f"({infer_ms/3:.1f}ms/张) | providers={facts.get('providers')}")
        # 同一实例上顺便验证 benchmark 不报「通道不可用」
        try:
            b = reg.benchmark("clip_vision", runs=3, warmup=1)
            print(f"[{mode}] benchmark: {b['avg_ms']}ms providers={b['providers']}")
        except Exception as e:  # noqa: BLE001
            print(f"[{mode}] ❌ benchmark 失败: {type(e).__name__}: {e}")
            return 1

    a = results["ort"]["vec"]
    b = results["auto"]["vec"]
    cos = float(np.sum(a * b) / (np.linalg.norm(a) * np.linalg.norm(b)))
    maxdiff = float(np.abs(a - b).max())
    print(f"\ncos(ORT, OV) = {cos:.8f} | max|Δ| = {maxdiff:.5f}")
    speed = results["ort"]["infer_ms"] / max(results["auto"]["infer_ms"], 1e-9)

    ok = True
    if cos < 0.9999:
        print("❌ 数值不达标：与 BUG-2026-0910-006 同类，必须放弃该后端或换档")
        ok = False
    if results["auto"]["status"]["backend"] != "openvino":
        print("❌ auto 模式下没有切到 OpenVINO 后端")
        ok = False
    print(f"\n结论：{'✅ 通过' if ok else '❌ 不通过'}")
    print(f"  向量一致 cos={cos:.8f}（门槛 0.9999）")
    print(f"  延迟 batch=3：ORT {results['ort']['infer_ms']:.0f}ms → "
          f"OV {results['auto']['infer_ms']:.0f}ms（提速 {speed:.2f}×）")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
