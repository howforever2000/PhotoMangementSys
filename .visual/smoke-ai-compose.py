"""冒烟：不依赖网络/模型，仅验证 Pillow 排版 + JPG/PDF 导出链路（FEAT-152）。

跑法：用带 Pillow 的 Python 执行
  python .visual/smoke-ai-compose.py
产物写到系统临时目录 ai_compose_smoke。
"""
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "python"))

from PIL import Image  # noqa: E402

from ai_studio import compose  # noqa: E402

src = os.path.join(ROOT, "night_city.jpg")
out = os.path.join(os.environ.get("TEMP", "."), "ai_compose_smoke")
os.makedirs(out, exist_ok=True)

img = Image.open(src).convert("RGB")

single = compose.export_journal([(img, "夜色温柔，灯火可亲。")], "blank", "城市夜色", out, "单图测试")
print("single:", single, [os.path.getsize(p) for p in single])

multi = compose.export_journal([(img, f"第{i + 1}页的配文，记录这一刻。") for i in range(3)], "film", "城市夜色", out, "多页测试")
print("multi:", multi, [os.path.getsize(p) for p in multi])

mag = compose.export_journal([(img, "杂志版式配文")], "magazine", "城市夜色", out, "杂志测试")
print("magazine:", mag)

pdf = [p for p in multi if p.lower().endswith(".pdf")][0]
with open(pdf, "rb") as f:
    head = f.read(5)
print("pdf header:", head)
assert head == b"%PDF-", "PDF 头不对"
assert all(os.path.getsize(p) > 0 for p in single + multi + mag)
print("SMOKE OK")
