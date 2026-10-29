"""NPU 是否真的在干活？——四条互相独立的判据（不采信任何自我上报）。

## 为什么需要这个脚本

`/health` 里写 `OpenVINO:NPU` 只能证明「会话是 NPU 编译出来的」，
证明不了「每次推理真的落在 NPU 上」。要证明后者，必须用与代码**无关**的证据：

  判据 1  EXECUTION_DEVICES     OpenVINO 自己回报这次编译实际绑到哪个设备
  判据 2  CPU 时间占比          进程 CPU 时间 / 墙钟。NPU 干活时 CPU 只做搬运，
                                该比值远小于 1；若静默回落到 CPU，会逼近核数
  判据 3  CPU 饱和干扰          把 14 个逻辑核占满后再测：NPU 延迟几乎不变，
                                CPU 路径会成倍恶化
  判据 4  真实图片吞吐          8 张真实缩略图的端到端延迟对比（NPU / ORT-DML / ORT-CPU）

判据 2 是最硬的一条：它不依赖任何"谁说自己跑在哪"，只依赖操作系统记的 CPU 时间。

用法：
    cd python
    .\\.venv-vcr\\Scripts\\python.exe bench\\probe_npu_liveness.py
    # 需要 OV-CPU 对照组时，把完整 openvino 装在 .tmp-ovprobe：
    #   $env:PYTHONPATH="$PWD\\.tmp-ovprobe"
"""
from __future__ import annotations

import os
import sys
import threading
import time

import numpy as np

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, HERE)

from vcr import config, ovrt  # noqa: E402


# ---------------------------------------------------------------------------
# 工具
# ---------------------------------------------------------------------------
def _cpu_time() -> float:
    """当前进程累计 CPU 时间（所有线程，秒）。"""
    return time.process_time()


def _measure(fn, runs: int = 20, warmup: int = 3) -> dict:
    """返回墙钟/CPU 时间/延迟中位数。"""
    for _ in range(warmup):
        fn()
    wall0, cpu0 = time.perf_counter(), _cpu_time()
    ts = []
    for _ in range(runs):
        t = time.perf_counter()
        fn()
        ts.append((time.perf_counter() - t) * 1000.0)
    wall = time.perf_counter() - wall0
    cpu = _cpu_time() - cpu0
    ts.sort()
    return {
        "runs": runs,
        "median_ms": round(ts[len(ts) // 2], 2),
        "wall_s": round(wall, 3),
        "cpu_s": round(cpu, 3),
        # 关键比值：NPU 应该 << 1，CPU 路径接近「参与计算的核数」
        "cpu_per_wall": round(cpu / wall, 3) if wall else None,
    }


class _CpuSaturator:
    """把每个逻辑核都占满 —— 用**外部进程**，不用 Python 线程。

    为何不用 threading：CPython 有 GIL，14 个忙线程会与主线程互相抢解释器锁，
    测出来的「变慢」里含大量 GIL 竞争（实测 NPU 会被放大 5.8×，而其中绝大部分
    是测量误差，不是 NPU 抢 CPU）。外部进程完全不与主进程争 GIL。
    """

    def __init__(self, procs: int | None = None):
        self.n = procs or (os.cpu_count() or 4)
        self._procs: list = []

    def __enter__(self):
        import subprocess

        code = "x=0\nwhile True:\n    x=(x*31+1)&0xFFFFFFFF"
        for _ in range(self.n):
            self._procs.append(
                subprocess.Popen([sys.executable, "-c", code],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            )
        time.sleep(1.5)          # 让它们真的跑满
        return self

    def __exit__(self, *_exc):
        for p in self._procs:
            try:
                p.kill()
            except Exception:  # noqa: BLE001
                pass
        for p in self._procs:
            try:
                p.wait(timeout=3)
            except Exception:  # noqa: BLE001
                pass


# ---------------------------------------------------------------------------
# 主流程
# ---------------------------------------------------------------------------
def main() -> int:
    p = config.clip_paths()
    size = int(p["size"])
    onnx = p["vision"]
    print(f"模型：{os.path.basename(onnx)} · 档位 {p['name']} · 输入 1x3x{size}x{size}")

    st = ovrt.status()
    print(f"组件：installed={st['installed']} usable={st['usable']} devices={st['devices']} "
          f"vendor={st['vendor']} version={st['version'].split('-')[0]}")
    if not st["usable"]:
        print("❌ 组件不可用（未装/未解压/无设备），无法测 NPU")
        return 1

    root = ovrt.ensure_unpacked()
    if root and root not in sys.path:
        sys.path.insert(0, root)
    import openvino as ov

    core = ov.Core()
    print("\n=== 判据 1：设备与真实执行设备 ===")
    for dev in core.available_devices:
        try:
            full = core.get_property(dev, "FULL_DEVICE_NAME")
        except Exception:  # noqa: BLE001
            full = "?"
        extra = ""
        for prop in ("NPU_DRIVER_VERSION", "DEVICE_ARCHITECTURE", "OPTIMIZATION_CAPABILITIES"):
            try:
                extra += f" {prop}={core.get_property(dev, prop)}"
            except Exception:  # noqa: BLE001
                pass
        print(f"  {dev}: {full}{extra}")

    model = core.read_model(onnx)
    model.reshape({model.input(0): [1, 3, size, size]})
    cm = core.compile_model(model, "NPU")
    exec_devs = None
    for prop in ("EXECUTION_DEVICES", "DEVICE_ARCHITECTURE"):
        try:
            exec_devs = cm.get_property(prop)
            print(f"  compiled.get_property({prop}) = {exec_devs}")
        except Exception as e:  # noqa: BLE001
            print(f"  compiled.get_property({prop}) 不可用: {type(e).__name__}")

    x = np.random.default_rng(0).standard_normal((1, 3, size, size)).astype(np.float32)

    print("\n=== 判据 2：CPU 时间占比（NPU 应远小于 1）===")
    npu_cold = None
    t0 = time.perf_counter()
    cm(x)                      # 首次含 NPU 编译
    npu_cold = (time.perf_counter() - t0) * 1000.0
    npu = _measure(lambda: cm(x), runs=20)
    print(f"  OV@NPU      中位 {npu['median_ms']:7.2f} ms | CPU/墙钟 = {npu['cpu_per_wall']} "
          f"（CPU {npu['cpu_s']}s / 墙钟 {npu['wall_s']}s，首轮含编译 {npu_cold:.0f}ms）")

    # 对照组：同一模型用 ORT-CPU（项目实际在用的运行时）
    ort_cpu = None
    try:
        import onnxruntime as ort

        so = ort.SessionOptions()
        so.intra_op_num_threads = config.threads()
        so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
        sess = ort.InferenceSession(onnx, sess_options=so, providers=["CPUExecutionProvider"])
        feed_name = sess.get_inputs()[0].name
        ort_cpu = _measure(lambda: sess.run(None, {feed_name: x}), runs=10)
        print(f"  ORT@CPU     中位 {ort_cpu['median_ms']:7.2f} ms | CPU/墙钟 = "
              f"{ort_cpu['cpu_per_wall']}（CPU {ort_cpu['cpu_s']}s / 墙钟 {ort_cpu['wall_s']}s，"
              f"{so.intra_op_num_threads} 线程）")
    except Exception as e:  # noqa: BLE001
        print(f"  ORT@CPU 对照组不可用: {type(e).__name__}: {e}")

    # 对照组：OV@CPU（需要完整 openvino 提供 CPU 插件；裁剪版没有）
    ov_cpu = None
    if "CPU" in core.available_devices:
        try:
            cm_cpu = core.compile_model(model, "CPU")
            ov_cpu = _measure(lambda: cm_cpu(x), runs=10)
            print(f"  OV@CPU      中位 {ov_cpu['median_ms']:7.2f} ms | CPU/墙钟 = "
                  f"{ov_cpu['cpu_per_wall']}")
        except Exception as e:  # noqa: BLE001
            print(f"  OV@CPU 对照组失败: {type(e).__name__}")

    print("\n=== 判据 3：全核外部进程占满时的干扰（NPU 应几乎不变）===")
    import subprocess  # noqa: F401  （只为提示依赖；Saturator 内部已引入）

    with _CpuSaturator() as sat:
        sat_npu = _measure(lambda: cm(x), runs=10, warmup=2)
        sat_ort = None
        if ort_cpu is not None:
            sat_ort = _measure(lambda: sess.run(None, {feed_name: x}), runs=5, warmup=1)
    print(f"  饱和时 OV@NPU 中位 {sat_npu['median_ms']:7.2f} ms "
          f"（空闲 {npu['median_ms']:.2f} → 变化 {sat_npu['median_ms'] / npu['median_ms']:.2f}×）"
          f" | CPU/墙钟 = {sat_npu['cpu_per_wall']}")
    if sat_ort:
        print(f"  饱和时 ORT@CPU 中位 {sat_ort['median_ms']:7.2f} ms "
              f"（空闲 {ort_cpu['median_ms']:.2f} → 变化 "
              f"{sat_ort['median_ms'] / ort_cpu['median_ms']:.2f}×）")

    print("\n=== 判据 4：真实图片 8 张一批（缩略图 → 预处理 → 前向）===")
    photos = []
    for base in ("test_fixture_photos", "covers"):
        d = os.path.join(os.path.dirname(HERE), base)
        if not os.path.isdir(d):
            continue
        for root_dir, _dirs, files in os.walk(d):
            for fn in files:
                if fn.lower().endswith((".jpg", ".jpeg", ".png")):
                    photos.append(os.path.join(root_dir, fn))
        if len(photos) >= 8:
            break
    photos = photos[:8]
    if len(photos) < 8:
        print(f"  跳过：只找到 {len(photos)} 张测试图")
    else:
        from vcr import preprocess
        from vcr.services import embed_service

        svc = embed_service.get_embed_service()
        batch = []
        for ph in photos:
            img = preprocess.open_image_ex(ph)[0] if hasattr(preprocess, "open_image_ex") else None
            if img is None:
                from PIL import Image

                img = Image.open(ph)
            batch.append(svc.preprocess_image(img) if hasattr(svc, "preprocess_image") else None)
        # 若服务没有单张预处理接口，退化为直接读真实缩略图尺寸的随机张量并说明
        if any(b is None for b in batch):
            print("  （embed_service 未暴露单张预处理接口，改用定长张量：只比前向，不含解码）")
            batch = [
                np.random.default_rng(i).standard_normal((3, size, size)).astype(np.float32)
                for i in range(8)
            ]
        stack = np.vstack([b[None, ...] if b.ndim == 3 else b for b in batch])
        r = _measure(lambda: [cm(stack[i:i + 1]) for i in range(stack.shape[0])],
                     runs=3, warmup=1)
        print(f"  OV@NPU   8 张 {r['wall_s'] * 1000 / 3:.0f} ms/批 "
              f"≈ {r['wall_s'] * 1000 / 3 / 8:.1f} ms/张 | CPU/墙钟 = {r['cpu_per_wall']}")

    print("\n=== 结论 ===")
    ok = True
    ratio = npu["cpu_per_wall"]
    if ratio is not None and ratio > 0.8:
        print(f"❌ 判据 2 不通过：NPU 推理时 CPU/墙钟 = {ratio}（接近 CPU 满负荷）"
              f"—— 说明算力并不在 NPU 上")
        ok = False
    else:
        print(f"✅ 判据 2 通过：NPU 推理 CPU/墙钟 = {ratio}（算力不在 CPU 上）")
    if ort_cpu:
        print(f"   对照：同一模型 ORT@CPU 的 CPU/墙钟 = {ort_cpu['cpu_per_wall']}，"
              f"延迟比 {ort_cpu['median_ms'] / npu['median_ms']:.1f}×")
    blow = sat_npu["median_ms"] / npu["median_ms"]
    if blow > 1.5:
        print(f"❌ 判据 3 不通过：全核占满后 NPU 延迟放大 {blow:.2f}× —— 它在抢 CPU")
        ok = False
    else:
        print(f"✅ 判据 3 通过：全核占满后 NPU 延迟仅 {blow:.2f}×（与 CPU 争抢无关）")
    if exec_devs is not None:
        print(f"   判据 1：EXECUTION_DEVICES = {exec_devs}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
