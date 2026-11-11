"""验证 edit_input_bytes / _fit_i2i_size 满足图生图面积约束。

网关约束：262144 (512x512) <= 面积 <= 6553600 (2560x2560)。

直接按文件路径加载 base.py，避免导入 ai_studio 包（会拉 fastapi 等重依赖）。
用法：C:\\Users\\Admin\\anaconda3\\python.exe python/bench/verify_i2i_area.py
"""
import importlib.util
import os
import sys
from io import BytesIO

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
BASE_PY = os.path.join(HERE, "..", "ai_studio", "workflows", "base.py")

MIN = 262144
MAX = 6553600


def load_base():
    spec = importlib.util.spec_from_file_location("ai_base_under_test", BASE_PY)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def check_dims(tag, w, h):
    ok = MIN <= w * h <= MAX
    print(f"{'OK ' if ok else 'BAD'} {tag}: -> {w}x{h} area={w * h}")
    return ok


def main():
    base = load_base()
    all_ok = True

    print("== _fit_i2i_size 纯尺寸计算 ==")
    cases = [
        (400, 556, 1024),   # 报错样张，面积 222400 偏小
        (300, 200, 1024),
        (512, 512, 1024),
        (1024, 768, 1024),
        (1200, 1600, 1024),
        (4000, 3000, 1024),
        (4000, 3000, 3072),
        (3072, 3072, 3072),  # 超分大图，面积过大需缩
        (800, 20000, 1024),  # 极端长条
    ]
    for w, h, mle in cases:
        nw, nh = base._fit_i2i_size(w, h, mle)
        all_ok &= check_dims(f"{w}x{h} mle={mle}", nw, nh)

    print("== edit_input_bytes 端到端（真实 PIL 编码）==")
    e2e = [(400, 556, 1024), (100, 80, 1024), (4000, 3000, 1024), (4000, 3000, 3072), (3072, 3072, 3072)]
    for w, h, mle in e2e:
        img = Image.new("RGB", (w, h), (120, 130, 140))
        out = base.edit_input_bytes(img, max_long_edge=mle, quality=88)
        res = Image.open(BytesIO(out))
        all_ok &= check_dims(f"edit {w}x{h} mle={mle}", res.width, res.height)
        assert res.format == "JPEG", res.format

    print("\nRESULT:", "ALL OK" if all_ok else "FAILED")
    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
