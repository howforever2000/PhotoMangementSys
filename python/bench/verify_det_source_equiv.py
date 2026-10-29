"""候选「人物检测」模型的等价性探针：比对本地 yolov8n-det.onnx 与待引入下载源的 onnx。

判据（任一不满足即不可作为下载源）：
  1. 输入/输出名与形状一致（宿主 python 侧按 out[0][0] → (84, 8400) 解码）
  2. 同一固定输入下输出最大绝对误差 < 1e-4（权重相同、仅容器差异）

用法：python .pi/probe_yolo_det.py <本地 onnx> <候选 onnx>
"""
import sys

import numpy as np
import onnxruntime as ort

# Windows 控制台默认 GBK，打印「✓ / ✅」会 UnicodeEncodeError 崩掉，先钉死 UTF-8
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def sig(path: str):
    s = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
    ins = [(i.name, i.shape, i.type) for i in s.get_inputs()]
    outs = [(o.name, o.shape, o.type) for o in s.get_outputs()]
    return s, ins, outs


def main() -> int:
    a, b = sys.argv[1], sys.argv[2]
    for tag, p in (("本地", a), ("候选", b)):
        _, ins, outs = sig(p)
        print(f"[{tag}] {p}")
        print(f"   in  = {ins}")
        print(f"   out = {outs}")

    sa, insa, outa = sig(a)
    sb, insb, outb = sig(b)
    ok = True
    if [i[1] for i in insa] != [i[1] for i in insb]:
        print("✗ 输入形状不一致")
        ok = False
    if [o[1] for o in outa] != [o[1] for o in outb]:
        print("✗ 输出形状不一致")
        ok = False

    rng = np.random.default_rng(0)
    x = rng.random((1, 3, 640, 640), dtype=np.float32)
    ya = sa.run(None, {insa[0][0]: x})[0]
    yb = sb.run(None, {insb[0][0]: x})[0]
    if ya.shape != yb.shape:
        print(f"✗ 前向输出形状不同: {ya.shape} vs {yb.shape}")
        return 1
    d = np.abs(ya.astype(np.float64) - yb.astype(np.float64))
    print(f"前向输出: shape={ya.shape} max|Δ|={d.max():.3e} mean|Δ|={d.mean():.3e}")
    if d.max() > 1e-4:
        print("✗ 输出误差过大（不是同一组权重）")
        ok = False
    print("✅ 等价" if ok else "❌ 不等价")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
