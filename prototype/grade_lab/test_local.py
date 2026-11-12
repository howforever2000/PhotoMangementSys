"""本地增强三功能的后端联调脚本（用 .venv-proto 跑，不依赖整套 FastAPI）。

只验证：LUT 查表 / 颜色迁移 / 三个 workflow.run 能产出文件。
"""
import os
import sys
import types
from types import SimpleNamespace

# providers.openai_compat 顶层 import requests；本测试不联网，桩掉即可（避免为联调装真实依赖）
sys.modules.setdefault("requests", types.ModuleType("requests"))

import numpy as np
from PIL import Image

sys.path.insert(0, os.path.abspath("python"))

from ai_studio import colortransfer, lutcube  # noqa: E402
from ai_studio.workflows import color_transfer, film_sim, restore  # noqa: E402

OUT = os.path.abspath("prototype/grade_lab/out/local_test")
os.makedirs(OUT, exist_ok=True)
SAMP = os.path.abspath("prototype/grade_lab/samples")
LUTS = os.path.abspath("python/ai_studio/luts")


def prog(p, s, m=""):
    print(f"  [{p}%] {s} {m}")


class C:
    def is_set(self):
        return False


def test_lut():
    arr = np.asarray(Image.open(os.path.join(SAMP, "night_city.jpg")).convert("RGB"), np.uint8)
    _, n = lutcube.get_cube(os.path.join(LUTS, "provia.cube"))
    out = lutcube.apply_cube(arr, os.path.join(LUTS, "provia.cube"), 0.9)
    Image.fromarray(out).save(os.path.join(OUT, "lut_provia.jpg"), quality=92)
    print("lut ok size", n, out.shape)


def test_ct():
    src = np.asarray(Image.open(os.path.join(SAMP, "night_city.jpg")).convert("RGB"), np.uint8)
    ref = np.asarray(Image.open(os.path.join(SAMP, "login_sunset.jpg")).convert("RGB"), np.uint8)
    out = colortransfer.reinhard_transfer(src, ref, strength=0.9)
    Image.fromarray(out).save(os.path.join(OUT, "ct_reinhard.jpg"), quality=92)
    txt = colortransfer.build_cube_text(ref, size=17)
    with open(os.path.join(OUT, "ref_17.cube"), "w", encoding="utf-8") as f:
        f.write(txt)
    print("ct ok", out.shape, "cube chars", len(txt))


def test_filmsim_wf():
    req = SimpleNamespace(
        outputDir=OUT,
        paths=[os.path.join(SAMP, "night_city.jpg")],
        options={"lut": "velvia", "strength": 1.0, "grain": 0.2, "vignette": 0.3},
        providers=None,
    )
    res = film_sim.run(req, prog, C())
    print("filmsim wf", res["outputs"])


def test_ct_wf():
    req = SimpleNamespace(
        outputDir=OUT,
        paths=[os.path.join(SAMP, "night_city.jpg")],
        options={
            "referencePath": os.path.join(SAMP, "login_sunset.jpg"),
            "method": "reinhard",
            "strength": 0.9,
            "exportCube": True,
        },
        providers=None,
    )
    res = color_transfer.run(req, prog, C())
    print("ct wf", res["outputs"], res["warnings"])


def test_restore_wf(face):
    sample = "face_adele.png" if face else "night_city.jpg"
    req = SimpleNamespace(
        outputDir=OUT,
        paths=[os.path.join(SAMP, sample)],
        options={"scale": 2, "face": face},
        providers=None,
    )
    res = restore.run(req, prog, C())
    print(f"restore wf face={face}", res["outputs"], res["warnings"])


if __name__ == "__main__":
    which = sys.argv[1] if len(sys.argv) > 1 else "all"
    if which in ("all", "lut"):
        test_lut()
    if which in ("all", "ct"):
        test_ct()
    if which in ("all", "film"):
        test_filmsim_wf()
    if which in ("all", "ctwf"):
        test_ct_wf()
    if which == "restore_face":
        test_restore_wf(True)
    if which == "restore_noface":
        test_restore_wf(False)
