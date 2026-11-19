"""云端「理解」调用输入尺寸的守卫（短边 1080）。

为什么钉这条：旧实现把 24MP 原图直接 `save(PNG)` 再 base64 发给视觉模型 ——
每次规划/复核实测 **5140ms / 37.9MB（base64 后 50.5MB）**，而模型根本不需要
这个分辨率。改成短边 1080 的 JPEG 后约 **0.2MB / 120ms**。

钉死的是**行为契约**而不是具体实现：
  1. 输出是 JPEG（魔数 `FF D8 FF`），不是 PNG；
  2. 短边 ≤ 1080，且**只缩不放**（小图不得被放大）；
  3. 长宽比保持（不裁切 —— 理解类调用要看全画面）。
"""
import io
import os
import sys
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

import numpy as np  # noqa: E402
from PIL import Image  # noqa: E402

from ai_studio.workflows import base  # noqa: E402


def _photo(w: int, h: int) -> Image.Image:
    """造一张有渐变的图（纯色会被 JPEG 压得过小，测不出真实体积）。

    用 numpy 一次生成：逐像素 `putpixel` 在 6000×4000 上要几十秒，
    这些用例的目的只是「验证缩放行为」，不该把测试耗时花在造图上。
    """
    yy = np.linspace(0, 255, h, dtype=np.float32)[:, None]
    xx = np.linspace(0, 255, w, dtype=np.float32)[None, :]
    arr = np.stack(
        [
            np.broadcast_to(xx, (h, w)),
            np.broadcast_to(yy, (h, w)),
            np.full((h, w), 128.0, dtype=np.float32),
        ],
        axis=-1,
    )
    return Image.fromarray(arr.astype(np.uint8))


class TestUnderstandBytes(unittest.TestCase):
    def test_large_photo_is_downscaled_to_short_edge(self):
        img = _photo(6000, 4000)  # 24MP，短边 4000
        data = base.understand_bytes(img)
        out = Image.open(io.BytesIO(data))
        self.assertEqual(min(out.size), base.UNDERSTAND_SHORT_EDGE)
        self.assertLess(min(out.size), 4000, "必须真的缩下来")

    def test_short_edge_is_the_constraint_not_long_edge(self):
        """竖图与横图都应让**短边**落到 1080 —— 按长边定档会让横图细节先糊。"""
        for w, h in ((4000, 6000), (6000, 4000), (4000, 4000)):
            data = base.understand_bytes(_photo(w, h))
            out = Image.open(io.BytesIO(data))
            self.assertEqual(min(out.size), base.UNDERSTAND_SHORT_EDGE, f"{w}x{h}")

    def test_aspect_ratio_preserved(self):
        """不裁切：理解类调用要看全画面（构图/位置关系是判断依据）。"""
        data = base.understand_bytes(_photo(6000, 4000))
        out = Image.open(io.BytesIO(data))
        self.assertAlmostEqual(out.size[0] / out.size[1], 6000 / 4000, places=2)

    def test_small_image_is_not_upscaled(self):
        """只缩不放：放大会更慢且不增加信息。"""
        img = _photo(320, 240)
        data = base.understand_bytes(img)
        out = Image.open(io.BytesIO(data))
        self.assertEqual(out.size, (320, 240))

    def test_output_is_jpeg(self):
        data = base.understand_bytes(_photo(2000, 1500))
        self.assertEqual(data[:3], b"\xff\xd8\xff", "必须是 JPEG 魔数")

    def test_much_smaller_than_png_on_photo_like_content(self):
        """照片类内容上体积必须显著小于 PNG 全图。

        注意不能拿「纯渐变」当样本：合成渐变图 PNG 只占 23KB（可完美预测），
        JPEG 反而更大。这条用带随机噪声的图模拟真实照片的高频细节，
        也同时验证「先缩到短边 1080」省掉的是像素数而不是压缩率。
        """
        rng = np.random.default_rng(7)
        w, h = 3000, 2000
        base_arr = np.stack(
            [
                np.linspace(0, 255, w, dtype=np.float32)[None, :].repeat(h, 0),
                np.linspace(0, 255, h, dtype=np.float32)[:, None].repeat(w, 1),
                np.full((h, w), 120.0, dtype=np.float32),
            ],
            axis=-1,
        )
        noise = rng.normal(0, 18, (h, w, 3)).astype(np.float32)
        img = Image.fromarray(np.clip(base_arr + noise, 0, 255).astype(np.uint8))

        png = base.png_bytes(img)
        small = base.understand_bytes(img)
        self.assertLess(len(small) * 5, len(png), f"jpeg={len(small)} png={len(png)}")

    def test_data_url_prefix_is_jpeg(self):
        url = base.understand_data_url(_photo(2000, 1500))
        self.assertTrue(url.startswith("data:image/jpeg;base64,"))


class TestImageDataUrlMime(unittest.TestCase):
    """providers 层必须按魔数标 mime：把 JPEG 标成 PNG 会被网关拒或解成花屏。"""

    def test_jpeg_bytes_get_jpeg_mime(self):
        from ai_studio.providers import openai_compat as oc

        jpeg = base.understand_bytes(_photo(1200, 900))
        url = oc._image_data_url(jpeg)
        self.assertTrue(url.startswith("data:image/jpeg;base64,"), url[:40])

    def test_png_bytes_still_get_png_mime(self):
        from ai_studio.providers import openai_compat as oc

        png = base.png_bytes(_photo(200, 150))
        url = oc._image_data_url(png)
        self.assertTrue(url.startswith("data:image/png;base64,"), url[:40])


if __name__ == "__main__":
    unittest.main()
