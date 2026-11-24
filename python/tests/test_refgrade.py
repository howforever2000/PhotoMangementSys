"""参考图调色·三层解耦单测（FEAT-219 §6 清单 + FEAT-220 覆盖率门控）。

钉死的语义：
  1. **恒等**：strength=0 与输入逐字节一致；
  2. **影调单调**：A1 分位曲线经前缀最大值强制单调，输出 L 不反转影调；默认锚定素材中位亮度；
  3. **分层无台阶**：软权重（色相双扇区插值 × 影调三角核）不留层边界台阶，5bit 色数不明显塌陷；
  4. **色域回收只缩彩度**：`_chroma_headroom` 不改色相角；
  5. **高光肩部等比缩放**：knee 以下逐字节不变，暖高光保彩度；
  6. **LUT 等价**：`eval_mapping` 直算 ≈ `build_lut` + `apply_cube`；`.cube` 往返互逆（轴序 [b,g,r]）；
  7. **多参考聚类**：色调分歧的参考被剔除；
  8. **肤色门控**：protectSkin 下肤色色度位移压到 35%；
  9. **旧路径零回归**：reinhard / histogram 输出哈希锁定（改动前录制）；
 10. **限幅**：A2+A3 合计 ‖Δab‖ ≤ 42、|ΔL| ≤ 22×toneWeight+12×0.45 的量级；
 11. **覆盖率门控（FEAT-220）**：覆盖度全 1 时与不门控等价；参考无覆盖的色相 A3 不作用；
 12. **门控不影响 A2**：门控只乘 A3 的残差。
"""
import hashlib
import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))

from ai_studio import colortransfer, lutcube, refgrade  # noqa: E402

# 旧路径 golden（FEAT-220 改动前录制；改 colortransfer 的数学会让它变红）
_GOLDEN_REINHARD_25 = "f413db8d8e350177"
_GOLDEN_HISTOGRAM_25 = "48a5ebdc595f3475"


# ---------------- 合成素材 ----------------

def _photo(seed: int = 7, h: int = 96, w: int = 128) -> np.ndarray:
    """有层次、有颜色的合成素材图：渐变 + 彩块 + 轻噪声。"""
    rng = np.random.default_rng(seed)
    yy, xx = np.mgrid[0:h, 0:w]
    base = (yy / float(h) * 0.8 + 0.1) * 255.0
    img = np.stack([base, base * 0.92, base * 0.85], axis=-1)
    img[h // 3 : 2 * h // 3, w // 4 : 3 * w // 4] = [210.0, 120.0, 90.0]
    img += rng.normal(0.0, 2.0, img.shape)
    return np.clip(img, 0, 255).astype(np.uint8)


def _tint(rgb: tuple[float, float, float], seed: int = 1, size: int = 64) -> np.ndarray:
    """指定主色调的合成参考图（带方差，统计量才有意义）。"""
    rng = np.random.default_rng(seed)
    img = np.empty((size, size, 3), dtype=np.float64)
    img[:] = rgb
    img += rng.normal(0.0, 6.0, img.shape)
    return np.clip(img, 0, 255).astype(np.uint8)


def _lch(lab: np.ndarray) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    L, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    return L, np.hypot(a, b), np.degrees(np.arctan2(b, a)) % 360.0


def _lab_of(rgb01: np.ndarray) -> np.ndarray:
    return colortransfer.rgb_to_lab(np.clip(rgb01 * 255.0 + 0.5, 0, 255).astype(np.uint8))


def _profile(*refs: np.ndarray) -> refgrade.RefProfile:
    return refgrade.build_profile(list(refs) if refs else [_tint((180, 120, 90))])


def _wheel_photo(size: int = 96) -> np.ndarray:
    """色相环素材：L 中调、C 固定，铺满色相轴 —— 让 A3 的残差格真正有数据。"""
    ang = np.linspace(0, 2 * np.pi, size, endpoint=False)
    lab = np.stack([
        np.full((size, size), 50.0, dtype=np.float32),
        (30.0 * np.cos(ang))[None, :].repeat(size, axis=0),
        (30.0 * np.sin(ang))[None, :].repeat(size, axis=0),
    ], axis=-1).astype(np.float32)
    rgb = np.clip(refgrade._lab_to_rgb01(lab.reshape(-1, 3)).reshape(lab.shape), 0, 1)
    return (rgb * 255.0 + 0.5).astype(np.uint8)


def _zero_residual_mapping(profile: refgrade.RefProfile, **kw) -> refgrade.Mapping:
    """只留 A1/A2 的映射：A3 残差清零，用于隔离门控与色域回收。"""
    base = refgrade.fit_mapping(_wheel_photo(), profile, **kw)
    return refgrade.Mapping(
        tone_xp=base.tone_xp, tone_fp=base.tone_fp, band_da=base.band_da, band_db=base.band_db,
        residual=np.zeros_like(base.residual), strength=base.strength,
        protect_skin=base.protect_skin, shoulder_mode=base.shoulder_mode,
        tone_weight=base.tone_weight, chroma_weight=base.chroma_weight, support=base.support,
    )


def _with_support(m: refgrade.Mapping, support: np.ndarray | None) -> refgrade.Mapping:
    """复用同一份映射参数，只替换门控（None = 不门控）。"""
    return refgrade.Mapping(
        tone_xp=m.tone_xp, tone_fp=m.tone_fp, band_da=m.band_da, band_db=m.band_db,
        residual=m.residual, strength=m.strength, protect_skin=m.protect_skin,
        shoulder_mode=m.shoulder_mode, tone_weight=m.tone_weight,
        chroma_weight=m.chroma_weight, support=support,
    )


class TestRefgradeCore(unittest.TestCase):
    def test_identity_strength_zero(self):
        src = _photo()
        out, _, _ = refgrade.transfer(src, _profile(), strength=0.0)
        self.assertTrue(np.array_equal(out, src), "strength=0 必须与输入逐字节一致")

    def test_tone_monotonic_and_anchored(self):
        prof = _profile()
        ramp = np.repeat(np.linspace(0, 255, 256, dtype=np.float32)[None, :, None], 64, axis=0)
        ramp_u8 = np.repeat(ramp, 3, axis=2).astype(np.uint8)
        m = refgrade.fit_mapping(ramp_u8, prof)
        out = refgrade.eval_mapping(m, ramp_u8.astype(np.float32) / 255.0, strength=1.0)
        Ls = _lab_of(ramp_u8.astype(np.float32) / 255.0)[..., 0]
        Lo = _lab_of(out)[..., 0]
        self.assertGreater(float(np.diff(Lo, axis=1).min()), -0.5, "输出 L 不得反转影调")
        self.assertLessEqual(abs(float(np.median(Lo) - np.median(Ls))), 2.0, "默认锚定素材中位亮度")

    def test_no_layer_step_banding(self):
        lab = np.zeros((64, 256, 3), dtype=np.float32)
        lab[..., 0] = np.linspace(5.0, 95.0, 256, dtype=np.float32)[None, :]
        lab[..., 1], lab[..., 2] = 20.0, 10.0
        ramp01 = np.clip(refgrade._lab_to_rgb01(lab.reshape(-1, 3)).reshape(lab.shape), 0, 1)
        ramp_u8 = (ramp01 * 255.0 + 0.5).astype(np.uint8)
        m = refgrade.fit_mapping(ramp_u8, _profile())
        out_u8 = lutcube.apply_cube(ramp_u8, refgrade.build_lut(m, 33))

        def uniq(a: np.ndarray) -> int:
            q = ((a.astype(np.int32) * 31 + 127) // 255)
            code = (q[..., 0] << 10) | (q[..., 1] << 5) | q[..., 2]
            return int((np.bincount(code.ravel(), minlength=1 << 15) > 0).sum())

        self.assertGreaterEqual(uniq(out_u8) / uniq(ramp_u8), 0.90, "软权重混合不应把色阶压塌")
        L = _lab_of(out_u8.astype(np.float32) / 255.0)[..., 0]
        self.assertLess(float(np.abs(np.diff(L, axis=1)).max()), 6.0, "相邻行 L 不应出现孤立大跳变")

    def test_gamut_recovery_keeps_hue(self):
        """直接喂越色域的合成 Lab 目标：headroom 之后经 sRGB 裁剪仍不应改色相角。

        零映射喂不进越色域目标（输入合法 sRGB 时 headroom 恒为 1），所以这里直接
        测收口机制本身 —— 这正是文档 §6 第 4 条要钉的性质。
        """
        ang = np.linspace(0, 2 * np.pi, 24, endpoint=False)
        lab = np.stack([np.full(24, 50.0), 120.0 * np.cos(ang), 120.0 * np.sin(ang)], axis=-1).astype(np.float32)
        cf = np.asarray(refgrade.autograde._chroma_headroom(lab)).reshape(-1)
        tgt = lab.copy()
        tgt[:, 1] *= cf
        tgt[:, 2] *= cf
        rgb = refgrade._lab_to_rgb01(tgt)
        self.assertTrue(np.all((rgb >= 0.0) & (rgb <= 1.0)), "色域回收后必须全部落在 [0,1]")
        back = _lab_of(rgb[None, :, :])[0]
        h_in = np.degrees(np.arctan2(tgt[:, 2], tgt[:, 1])) % 360.0
        h_out = np.degrees(np.arctan2(back[:, 2], back[:, 1])) % 360.0
        drift = np.abs((h_out - h_in + 180.0) % 360.0 - 180.0)
        self.assertLessEqual(float(drift.max()), 1.5, "色域回收只缩 C，不该改色相角")

    def test_shoulder_ratio(self):
        low = np.array([[0.5, 0.3, 0.2]], dtype=np.float32)
        self.assertTrue(np.array_equal(refgrade._shoulder_ratio(low), low), "knee 以下必须逐字节不变")
        warm = np.array([[1.0, 0.92, 0.66]], dtype=np.float32)
        out = refgrade._shoulder_ratio(warm)
        _, c_in, _ = _lch(_lab_of(warm[None, :, :])[0])
        _, c_out, _ = _lch(_lab_of(out[None, :, :])[0])
        self.assertGreaterEqual(float(c_out[0] / c_in[0]), 0.90, "等比肩部应保住高光彩度")
        self.assertLess(float(out.max()), 0.97, "顶端应被压到 target 以下")

    def test_lut_equivalence_and_cube_roundtrip(self):
        src = _photo()
        m = refgrade.fit_mapping(src, _profile())
        lut, n = refgrade.build_lut(m, 33)
        direct = refgrade.eval_mapping(m, src.astype(np.float32) / 255.0, strength=1.0)
        via_lut = lutcube.apply_cube(src, (lut, n))
        self.assertLessEqual(float(np.abs(direct * 255.0 - via_lut).mean()), 1.5, "LUT 插值误差应在 1.5 级内")

        text = colortransfer.cube_text_from_lut(lut, n)
        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / "t.cube"
            p.write_text(text, encoding="utf-8")
            back, n_back = lutcube.load_cube(str(p))
        self.assertEqual(n_back, n)
        self.assertTrue(np.allclose(back, lut, atol=1e-5), ".cube 往返必须互逆（轴序 [b,g,r]）")

    def test_multi_ref_clustering(self):
        prof = _profile(_tint((190, 130, 80), 1), _tint((185, 125, 85), 2), _tint((70, 110, 175), 3))
        self.assertEqual(prof.kept, (0, 1), "暖色两张应成最大簇")
        self.assertEqual(prof.dropped, (2,), "冷色参考应被剔除")
        self.assertEqual(_profile(_tint((180, 120, 90))).dropped, (), "单张参考直通，零剔除")

    def test_skin_gate(self):
        prof = _profile(_tint((200, 140, 95)))
        skin_lab = np.array([[65.0, 25.0 * np.cos(np.radians(40)), 25.0 * np.sin(np.radians(40))]], dtype=np.float32)
        skin = np.clip(refgrade._lab_to_rgb01(skin_lab), 0, 1).astype(np.float32)[None, :, :]
        chunk = np.repeat(skin, 16, axis=1)

        def shift(protect: bool) -> float:
            m = refgrade.fit_mapping(_photo(), prof, protect_skin=protect)
            out = refgrade.eval_mapping(m, chunk, strength=1.0)
            _, c_s, _ = _lch(_lab_of(chunk)[0])
            _, c_o, _ = _lch(_lab_of(out)[0])
            return float(abs(c_o - c_s).mean())

        self.assertLessEqual(shift(True), shift(False) * 0.45, "protectSkin 应把肤色色度位移压到 35% 量级")

    def test_legacy_paths_golden(self):
        src = _photo()
        ref = _tint((180, 120, 90))
        merged = colortransfer.merge_reference_stats([colortransfer.reference_stats(ref)])
        a = colortransfer.reinhard_transfer_stats(src, merged, l_weight=1.0, strength=0.25)
        b = colortransfer.reinhard_transfer_stats(src, merged, l_weight=1.0, strength=0.25)
        self.assertTrue(np.array_equal(a, b), "旧路径必须确定性")
        self.assertEqual(hashlib.sha256(np.ascontiguousarray(a).tobytes()).hexdigest()[:16],
                         _GOLDEN_REINHARD_25, "reinhard 输出已漂移，旧配方回归")
        h = colortransfer.histogram_match_sorted(src, colortransfer.histogram_ref_sorted([ref]), 0.25)
        self.assertEqual(hashlib.sha256(np.ascontiguousarray(h).tobytes()).hexdigest()[:16],
                         _GOLDEN_HISTOGRAM_25, "histogram 输出已漂移")

    def test_limits(self):
        """两层限幅的两种口径：

        ① 结构性：拟合产物本身必须已夹到上限（A2 ‖Δab‖≤30、A3 ‖Δab‖≤12×0.45、|ΔL|≤12×0.45）；
        ② 渲染性：喂 C≤60 的中低饱和色时，sRGB 往返损失可忽略，实测位移仍应 ≤42。
           输入用已在色域边界上的高饱和色（如纯蓝 C≈134）时，headroom 会把彩度拉回，
           测到的是“被裁剪后的样子”而不是限幅失效，故不纳入断言。
        """
        prof = _profile(_tint((190, 130, 80)))
        m = refgrade.fit_mapping(_wheel_photo(), prof, tone_weight=0.5)

        band_norm = np.hypot(m.band_da, m.band_db)
        self.assertLessEqual(float(band_norm.max()), refgrade._CHROMA_CAP + 1e-4, "A2 未限幅")
        res_ab = np.linalg.norm(m.residual[..., 1:], axis=-1)
        self.assertLessEqual(float(res_ab.max()), refgrade._RES_CHROMA_CAP * refgrade._RES_WEIGHT + 1e-4, "A3 未限幅")
        self.assertLessEqual(float(np.abs(m.residual[..., 0]).max()),
                             refgrade._RES_L_CAP * refgrade._RES_WEIGHT + 1e-4, "A3 ΔL 未限幅")

        rng = np.random.default_rng(11)
        raw = rng.random((4000, 3), dtype=np.float32)
        lab = _lab_of(raw[None, :, :])[0]
        keep = (lab[:, 0] > 25.0) & (lab[:, 0] < 75.0) & (np.hypot(lab[:, 1], lab[:, 2]) < 60.0)
        colors = raw[keep]
        self.assertGreater(colors.shape[0], 500, "测试样本不足")

        out = refgrade.eval_mapping(m, colors[None, :, :], strength=1.0)[0]
        s, o = _lab_of(colors[None, :, :])[0], _lab_of(out[None, :, :])[0]
        self.assertLessEqual(float(np.linalg.norm(o[:, 1:] - s[:, 1:], axis=-1).max()), 42.0,
                             "A2+A3 合计 ‖Δab‖ 超限")
        self.assertLessEqual(float(np.abs(o[:, 0] - s[:, 0]).max()),
                             22.0 * 0.5 + 12.0 * refgrade._RES_WEIGHT + 1.5, "ΔL 超限")


class TestCoverageGate(unittest.TestCase):
    """FEAT-220：覆盖率门控只影响 A3，且只在参考图没有覆盖的色相上生效。"""

    def test_full_coverage_equals_no_gate(self):
        src = _photo()
        base = refgrade.fit_mapping(_wheel_photo(), _profile())
        src01 = src.astype(np.float32) / 255.0
        none_ = _with_support(base, None)
        ones = _with_support(base, np.ones((3, refgrade._SUPPORT_BINS), dtype=np.float32))
        gated = _with_support(base, base.support)
        self.assertLessEqual(
            float(np.abs(refgrade.eval_mapping(ones, src01) - refgrade.eval_mapping(none_, src01)).max()),
            1e-5, "覆盖度全 1 时必须与不门控等价",
        )
        self.assertGreater(
            float(np.abs(refgrade.eval_mapping(gated, src01) - refgrade.eval_mapping(none_, src01)).max()),
            1e-4, "真实覆盖度下门控必须真的起作用（否则等于没实现）",
        )

    def test_uncovered_hue_is_not_touched_by_a3(self):
        """暖色参考 + 蓝色素材：门控后 A3 应几乎不作用，输出贴近「只有 A1/A2」。"""
        blue = np.repeat(np.array([[[70.0, 140.0, 225.0]]], dtype=np.float32) / 255.0, 32, axis=1)
        blue = np.repeat(blue, 8, axis=0)
        prof = _profile(_tint((200, 140, 90)))
        wheel = _wheel_photo()

        only_a2 = _zero_residual_mapping(prof)
        full = refgrade.fit_mapping(wheel, prof)
        self.assertGreater(float(np.abs(full.residual).max()), 0.0, "测试前提：A3 残差必须非零")
        ungated = _with_support(full, np.ones((3, refgrade._SUPPORT_BINS), dtype=np.float32))
        base = _lab_of(refgrade.eval_mapping(only_a2, blue))
        d_gated = float(np.linalg.norm(_lab_of(refgrade.eval_mapping(full, blue)) - base, axis=-1).mean())
        d_raw = float(np.linalg.norm(_lab_of(refgrade.eval_mapping(ungated, blue)) - base, axis=-1).mean())
        self.assertGreater(d_raw, 0.5, "测试前提：不门控时 A3 对蓝天的确有作用")
        self.assertLess(d_gated, d_raw * 0.5, "门控后 A3 对参考未覆盖的色相应几乎不作用")

    def test_gate_leaves_a2_untouched(self):
        src = _photo()
        prof = _profile()
        m = _zero_residual_mapping(prof)          # residual 全 0，只剩 A1/A2
        no_gate = _with_support(m, None)
        src01 = src.astype(np.float32) / 255.0
        self.assertLessEqual(
            float(np.abs(refgrade.eval_mapping(m, src01) - refgrade.eval_mapping(no_gate, src01)).max()),
            1e-6, "门控不得影响 A2 分区色偏",
        )

    def test_support_shape_and_coverage(self):
        prof = _profile()
        self.assertEqual(prof.support.shape, (3, refgrade._SUPPORT_BINS))
        self.assertTrue(np.all((prof.support >= 0.0) & (prof.support <= 1.0)))
        self.assertLessEqual(refgrade.support_coverage(prof.support), 1.0)
        self.assertEqual(refgrade.support_coverage(None), 1.0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
