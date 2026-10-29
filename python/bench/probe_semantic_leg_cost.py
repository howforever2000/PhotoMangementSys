"""语义腿在三种后端下的**真实端到端**成本（走 HTTP /embed_batch，含解码与落库前处理）。

目的：回答「NPU 到底给扫描省了多少」。前面 probe_npu_liveness.py 只证明 NPU 在算、
且比 CPU 快；本脚本把同一批真实照片分别喂给三种后端，量出「每张多少钱」，
再与宿主日志里的 scan.timing 各段对比，算出它占扫描总时长的比例。

用法（需要先起一个服务实例）：
    $env:VCR_PORT=8802; .\\.venv-vcr\\Scripts\\python.exe server.py
    .\\.venv-vcr\\Scripts\\python.exe bench\\probe_semantic_leg_cost.py 8802
"""
from __future__ import annotations

import json
import os
import sys
import time
import urllib.request

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PROJ = os.path.dirname(HERE)


def post(port: int, path: str, payload: dict, timeout: float = 300) -> dict:
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


def get(port: int, path: str, timeout: float = 60) -> dict:
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=timeout) as r:
        return json.loads(r.read().decode())


def collect_photos(n: int = 8) -> list[str]:
    out: list[str] = []
    for base in ("test_fixture_photos", "covers"):
        d = os.path.join(PROJ, base)
        if not os.path.isdir(d):
            continue
        for root, _dirs, files in os.walk(d):
            for fn in files:
                if fn.lower().endswith((".jpg", ".jpeg", ".png")):
                    out.append(os.path.join(root, fn))
            if len(out) >= n:
                return out[:n]
    return out[:n]


def run_case(port: int, label: str, gpu: bool, runtime: str, photos: list[str]) -> dict | None:
    post(port, "/gpu", {"enabled": gpu})
    post(port, "/clip_runtime", {"mode": runtime})
    # 触发一次加载（切换后是后台重建，等它就绪）
    deadline = time.time() + 180
    while time.time() < deadline:
        h = get(port, "/health")
        if h.get("clip_ready"):
            break
        time.sleep(0.5)
    h = get(port, "/health")
    if not h.get("clip_ready"):
        print(f"[{label}] ❌ 未就绪: load_errors={h.get('load_errors')}")
        return None
    sess = (h.get("gpu", {}).get("sessions") or {}).get("clip_vision")
    rt = h.get("clip_runtime", {})

    # 预热 1 轮（含可能的 NPU 编译），再测 3 轮取最快
    post(port, "/embed_batch", {"paths": photos})
    best = None
    for _ in range(3):
        t = time.perf_counter()
        r = post(port, "/embed_batch", {"paths": photos})
        ms = (time.perf_counter() - t) * 1000.0
        n = len(r.get("results", []))
        if best is None or ms < best[0]:
            best = (ms, n)
    print(f"[{label}] clip_vision={sess} backend={rt.get('backend')} "
          f"device={rt.get('device') or '-'} | {best[1]} 张 {best[0]:.0f} ms "
          f"→ {best[0] / max(best[1], 1):.1f} ms/张")
    return {"label": label, "providers": sess, "ms": best[0], "n": best[1],
            "per_photo": best[0] / max(best[1], 1)}


def main() -> int:
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8802
    photos = collect_photos(8)
    if len(photos) < 8:
        print(f"测试图不足：{len(photos)}")
        return 1
    print(f"用 {len(photos)} 张真实图片（同一批、同一进程、交替切换后端）\n")

    cases = [
        ("OV @ NPU", False, "auto"),        # GPU 关掉，排除 DML 干扰
        ("ORT @ CPU", False, "ort"),
        ("ORT @ DML", True, "ort"),
    ]
    rows = []
    for label, gpu, runtime in cases:
        r = run_case(port, label, gpu, runtime, photos)
        if r:
            rows.append(r)

    if not rows:
        return 1
    base = next((r for r in rows if r["label"] == "ORT @ CPU"), None)
    npu = next((r for r in rows if r["label"] == "OV @ NPU"), None)
    print("\n=== 结论 ===")
    for r in rows:
        print(f"  {r['label']:12s} {r['per_photo']:6.1f} ms/张")
    if base and npu:
        print(f"\n  NPU vs CPU: {base['per_photo'] / npu['per_photo']:.2f}× 提速")
    print(
        "\n  对照扫描总时长（app.log album=126 / 776 张）:\n"
        "    AI识别   99049 ms  (79%)\n"
        "    影调     26539 ms  (21%)\n"
        "    语义向量   224 ms  (0.2%)  ← NPU 只能影响这一段"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
