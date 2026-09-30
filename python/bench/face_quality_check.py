# 质量闸门单测（纯函数，用合成关键点；跑在 .venv-vcr 下）
import os
import sys

import numpy as np

sys.path.insert(0, r"D:\YUAN HAO\Documents\PersonalIdeaProject\PhotoMangementSys\python")
from vcr import config  # noqa: E402
from vcr.services import face_service as fs  # noqa: E402

fail = 0


def check(name, got, want):
    global fail
    ok = got == want
    if not ok:
        fail += 1
    print(f"  [{'OK ' if ok else 'FAIL'}] {name}: {got} (期望 {want})")


print("阈值:", dict(det_conf=config.FACE_DET_CONF, min_pix=config.FACE_MIN_PIX,
                  eye_span=config.FACE_QUALITY_EYE_SPAN, blur=config.FACE_QUALITY_BLUR_MIN,
                  marginal_pix=config.FACE_MARGINAL_MIN_PIX))

# 1) 正常人脸：框 200×200，眼距 80(0.40W)，鼻在眼中点下 0.6*eye_d，嘴再下 0.5*eye_d
W = 200
eye_d = 0.40 * W
le, re_ = np.array([60.0, 60.0]), np.array([60.0 + eye_d, 60.0])
nose = np.array([100.0, 60.0 + 0.6 * eye_d])
ml = np.array([80.0, nose[1] + 0.5 * eye_d])
mr = np.array([120.0, nose[1] + 0.5 * eye_d])
normal = np.stack([le, re_, nose, ml, mr])
check("正常大脸（几何+尺寸都过）", fs.face_geometry_quality((100, 50, 300, 250), normal), fs.QUALITY_OK)

# 2) 垃圾框：眼距极小、鼻远在下方（后脑勺/风扇那类）
bad = np.stack([np.array([100.0, 60.0]), np.array([104.0, 60.0]), np.array([102.0, 300.0]),
                np.array([100.0, 600.0]), np.array([104.0, 600.0])])
check("垃圾框（眼距/鼻位都不像人脸）", fs.face_geometry_quality((100, 50, 300, 250), bad), fs.QUALITY_REJECT)

# 3) 小脸：短边 40 < FACE_MIN_PIX=48
small = np.stack([le, re_, nose, ml, mr]) * 0.2
check("小脸（短边 40px）", fs.face_geometry_quality((0, 0, 40, 40), small), fs.QUALITY_REJECT)

# 4) 边缘质量：几何合法但 yaw≈0.4（侧脸）
side = np.stack([np.array([60.0, 60.0]), np.array([140.0, 60.0]), np.array([132.0, 120.0]),
                 np.array([80.0, 170.0]), np.array([120.0, 170.0])])
check("侧脸 yaw=0.40（应判边缘质量）", fs.face_geometry_quality((100, 50, 300, 250), side), fs.QUALITY_MARGINAL)

# 5) 边缘质量：小脸但 ≥MIN_PIX（短边 50 < 64）
kps50 = np.stack([le, re_, nose, ml, mr]) * 0.25
check("短边 50px（应判边缘质量）", fs.face_geometry_quality((0, 0, 50, 50), kps50), fs.QUALITY_MARGINAL)

# 6) 宽高比离谱（细长框）
check("细长框 300×100", fs.face_geometry_quality((0, 0, 300, 100), normal), fs.QUALITY_REJECT)

# 7) 清晰度：全灰图 = 0 方差 → 必然低于门槛
flat = np.full((1, 3, 112, 112), -127.5 / 128.0, dtype=np.float32)
check("全灰对齐图（blur≈0）", fs.aligned_blur(flat) < config.FACE_QUALITY_BLUR_MIN, True)

# 8) 清晰度：棋盘格 = 高方差 → 必然高于门槛
cb = np.zeros((112, 112), dtype=np.float32)
cb[::2, ::2] = 255.0
cb[1::2, 1::2] = 255.0
t = np.stack([cb, cb, cb])[None, ...]
t = (t - 127.5) / 128.0
check("棋盘格对齐图（blur 高）", fs.aligned_blur(t.astype(np.float32)) >= config.FACE_QUALITY_BLUR_MIN, True)

print("\n[OK] 质量闸门单测通过" if fail == 0 else f"\n[FAIL] {fail} 项未通过")
sys.exit(1 if fail else 0)
