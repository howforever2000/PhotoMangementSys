"""服务层：夜景影调通道（Phase 1）

背景：Rust 侧 tone.rs 的 scan_album_tones 是独立按钮，未接入 classify 流水线
（Python /classify 请求不携带 tone 数据）。夜景判定必须在 Python 端自算影调，
不改 Rust→Python 接口契约。

方法与 tone.rs 完全一致：
  等比下采样到 256px → BT.601 整数定点加权亮度 (299R+587G+114B)/1000 → avg_luma

性能：256px 下采样 + 均值 ≈ 3~6ms（验收 ≤30ms）。
"""
from dataclasses import dataclass

import numpy as np
from PIL import Image

SAMPLE_SIZE = 256


@dataclass
class ToneOutcome:
    avg_luma: float | None = None
    ready: bool = False
    error: str = ""


def compute_tone(img: Image.Image) -> ToneOutcome:
    """返回整图平均亮度（0~255）。失败时 ready=False（夜景通道自动降级）。"""
    try:
        w, h = img.size
        if w <= 0 or h <= 0:
            return ToneOutcome(error="无效尺寸")
        r = SAMPLE_SIZE / max(w, h)
        small = img.resize((max(1, round(w * r)), max(1, round(h * r))), Image.BILINEAR)
        rgb = np.asarray(small.convert("RGB"), dtype=np.uint32)
        # BT.601 加权（整数定点，与 src-tauri/src/tone.rs 一致）
        luma = (299 * rgb[..., 0] + 587 * rgb[..., 1] + 114 * rgb[..., 2]) // 1000
        return ToneOutcome(avg_luma=float(luma.mean()), ready=True)
    except Exception as e:  # noqa: BLE001
        return ToneOutcome(error=str(e))
