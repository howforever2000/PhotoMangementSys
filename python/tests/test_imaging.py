"""图像算法的回归测试：压缩 / 调色 / 人像 / 排版。

覆盖 docs/v5.md §20 里点名的用例，另外补两个本项目特有的雷：
- **中文路径**（Pillow 传字符串路径在某些版本会静默写 0 字节）；
- **透明底铺白**（PNG 直接存 JPEG 会变黑底）。
"""
from __future__ import annotations

import random
from pathlib import Path

import pytest
from PIL import Image

from agent.imaging import beauty, probe
from agent.imaging.compress import compress, parse_crop
from agent.imaging.imageio import ImageIoError, save_jpeg
from agent.imaging.journal import export_cards, render_pages
from agent.imaging.style import apply_style


def make_noise_image(path: Path, size=(2400, 1600), seed: int = 7) -> Path:
    """造一张"难压"的图（随机噪点最难压，能可靠触发缩边逻辑）。"""
    rnd = random.Random(seed)
    w, h = size
    data = bytes(rnd.getrandbits(8) for _ in range(w * h * 3))
    img = Image.frombytes("RGB", (w, h), data)
    path.parent.mkdir(parents=True, exist_ok=True)
    save_jpeg(img, str(path), quality=95)
    return path


def make_flat_image(path: Path, size=(1200, 900), color=(180, 150, 120), mode="RGB") -> Path:
    img = Image.new(mode, size, color)
    if mode == "RGBA":
        # 左半边透明
        for x in range(size[0] // 2):
            for y in range(0, size[1], 7):
                img.putpixel((x, y), (255, 0, 0, 0))
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(str(path))
    return path


# ------------------------------------------------------------------ 压缩


def test_compress_hits_target_kb(tmp_path: Path):
    src = make_noise_image(tmp_path / "big.jpg")
    dst = str(tmp_path / "out" / "big_c.jpg")
    r = compress(str(src), dst, target_kb=120, max_edge=2048)
    assert r.ok, r.note
    assert r.bytes <= 120 * 1024
    assert Path(dst).stat().st_size == r.bytes
    assert r.width <= 2048 and r.height <= 2048


def test_compress_reports_failure_instead_of_lying(tmp_path: Path):
    # 目标 5KB 的噪点图基本不可能达成（缩到最小还是超），必须 ok=False + 人话说明
    src = make_noise_image(tmp_path / "huge.jpg", size=(3000, 2000))
    r = compress(str(src), str(tmp_path / "huge_c.jpg"), target_kb=5, max_edge=0)
    if not r.ok:
        assert "仍超过目标" in r.note
    else:  # 极少数实现能把噪点压到 5KB，那也算达标
        assert r.bytes <= 5 * 1024


def test_compress_respects_max_edge_only_downscale(tmp_path: Path):
    src = make_flat_image(tmp_path / "small.png", size=(600, 400))
    r = compress(str(src), str(tmp_path / "small_c.jpg"), max_edge=2000)
    assert (r.width, r.height) == (600, 400), "只缩不放"


def test_compress_crop_ratio(tmp_path: Path):
    src = make_flat_image(tmp_path / "wide.png", size=(1600, 800))
    r = compress(str(src), str(tmp_path / "wide_c.jpg"), crop="1:1")
    assert r.width == r.height
    assert r.width == 800


def test_parse_crop_accepts_and_rejects():
    assert parse_crop("4:3") == pytest.approx(4 / 3)
    assert parse_crop("") is None
    assert parse_crop("garbage") is None
    assert parse_crop("0:1") is None


def test_transparent_png_gets_white_background(tmp_path: Path):
    src = make_flat_image(tmp_path / "rgba.png", size=(400, 300), mode="RGBA", color=(0, 0, 0, 0))
    out = str(tmp_path / "rgba_out.jpg")
    compress(str(src), out)
    with Image.open(out) as img:
        assert img.mode == "RGB"
        # 全透明 → 铺白（不是黑底）
        assert img.getpixel((5, 5))[0] > 240


def test_chinese_path_works(tmp_path: Path):
    # 本仓库踩过的坑：中文路径下 Pillow 传字符串保存会写出 0 字节
    d = tmp_path / "中文目录"
    src = make_flat_image(d / "杭州·2023 秋.png", size=(800, 600))
    dst = str(d / "输出_压缩.jpg")
    r = compress(str(src), dst, target_kb=200)
    assert Path(dst).stat().st_size > 0
    assert probe.inspect(dst)["ok"]


def test_original_bytes_unchanged(tmp_path: Path):
    src = make_noise_image(tmp_path / "keep.jpg")
    before = src.read_bytes()
    compress(str(src), str(tmp_path / "keep_c.jpg"), target_kb=80)
    assert src.read_bytes() == before, "压缩不得改动原图"


def test_missing_source_raises_human_error(tmp_path: Path):
    with pytest.raises(ImageIoError) as ei:
        compress(str(tmp_path / "nope.jpg"), str(tmp_path / "x.jpg"))
    assert "不存在" in str(ei.value)


# ------------------------------------------------------------------ 调色


def test_style_is_deterministic(tmp_path: Path):
    src = make_flat_image(tmp_path / "a.png", size=(320, 240), color=(120, 140, 170))
    out1, out2 = str(tmp_path / "a1.jpg"), str(tmp_path / "a2.jpg")
    r1 = apply_style(str(src), out1, "warm_dusk", "moderate")
    r2 = apply_style(str(src), out2, "warm_dusk", "moderate")
    assert Path(out1).read_bytes() == Path(out2).read_bytes(), "同输入同参数必须同输出"
    assert r1.params == r2.params


def test_style_warm_dusk_moves_toward_warm(tmp_path: Path):
    src = make_flat_image(tmp_path / "gray.png", size=(64, 64), color=(128, 128, 128))
    out = str(tmp_path / "warm.jpg")
    apply_style(str(src), out, "warm_dusk", "strong")
    with Image.open(out) as img:
        r, g, b = img.getpixel((10, 10))
    assert r > b, "暖调预设应让红色通道高于蓝色通道"


def test_style_intensity_changes_output(tmp_path: Path):
    src = make_flat_image(tmp_path / "flat.png", size=(64, 64), color=(110, 130, 160))
    a, b = str(tmp_path / "s.jpg"), str(tmp_path / "s.png".replace("png", "st.jpg"))
    apply_style(str(src), a, "warm_dusk", "subtle")
    apply_style(str(src), b, "warm_dusk", "strong")
    assert Path(a).read_bytes() != Path(b).read_bytes()


# ------------------------------------------------------------------ 人像


def test_beauty_keeps_size_and_is_deterministic(tmp_path: Path):
    # 造一块"肤色"区域（YCbCr 落在肤色区间内）
    img = Image.new("RGB", (240, 240), (35, 60, 90))
    for x in range(60, 180):
        for y in range(60, 180):
            img.putpixel((x, y), (220, 175, 150))
    src = tmp_path / "skin.png"
    img.save(str(src))
    o1, o2 = str(tmp_path / "r1.jpg"), str(tmp_path / "r2.jpg")
    beauty.smooth(str(src), o1, "natural")
    beauty.smooth(str(src), o2, "natural")
    assert Path(o1).read_bytes() == Path(o2).read_bytes()
    with Image.open(o1) as out:
        assert out.size == (240, 240)


# ------------------------------------------------------------------ 排版 / PDF


def test_cards_page_count_is_deterministic(tmp_path: Path):
    paths = [str(make_flat_image(tmp_path / f"p{i}.png", size=(800, 600))) for i in range(7)]
    pages_a, font_a, used_a = render_pages(paths, layout="magazine_3up", title="杭州 · 2023", subtitle="秋")
    pages_b, font_b, used_b = render_pages(paths, layout="magazine_3up", title="杭州 · 2023", subtitle="秋")
    assert len(pages_a) == len(pages_b) == 3, "7 张 / 每页 3 张 → 3 页"
    assert used_a == used_b == 7
    assert font_a == font_b
    assert font_a != "none", "必须能解析到字体（否则卡片上的中文会变方块）"
    # 逐字节相同（确定性排版）
    for pa, pb in zip(pages_a, pages_b):
        assert pa.tobytes() == pb.tobytes()


def test_cards_pdf_export(tmp_path: Path):
    paths = [str(make_flat_image(tmp_path / f"q{i}.png", size=(600, 400))) for i in range(4)]
    res = export_cards(paths, str(tmp_path / "out"), layout="magazine_3up", formats=["pdf"], max_pages=12)
    assert res.pdf and Path(res.pdf).is_file()
    assert Path(res.pdf).read_bytes()[:4] == b"%PDF"
    assert res.page_count == 2


def test_cards_respect_max_pages(tmp_path: Path):
    paths = [str(make_flat_image(tmp_path / f"m{i}.png", size=(400, 300))) for i in range(30)]
    res = export_cards(paths, str(tmp_path / "out2"), layout="magazine_3up", formats=["pdf"], max_pages=2)
    assert res.page_count == 2
    assert res.fitted == 24, "超出页数上限的张数应如实上报"


def test_cards_survive_broken_image(tmp_path: Path):
    good = str(make_flat_image(tmp_path / "ok.png", size=(400, 300)))
    bad = str(tmp_path / "broken.jpg")
    Path(bad).write_bytes(b"not an image")
    res = export_cards([good, bad], str(tmp_path / "out3"), layout="magazine_3up", formats=["pdf"])
    assert res.page_count == 1, "坏图不该毁掉整页"


# ------------------------------------------------------------------ 体检


def test_probe_flags_zero_byte_file(tmp_path: Path):
    p = tmp_path / "empty.jpg"
    p.write_bytes(b"")
    info = probe.inspect(str(p))
    assert info["ok"] is False
    assert "0 字节" in info["error"]


def test_probe_sharpness_drops_after_blur(tmp_path: Path):
    from PIL import ImageFilter

    src = make_noise_image(tmp_path / "sharp.jpg", size=(400, 300))
    blurred = Image.open(src).filter(ImageFilter.GaussianBlur(radius=4))
    bpath = str(tmp_path / "blur.jpg")
    save_jpeg(blurred, bpath, quality=92)
    assert probe.sharpness(str(src)) > probe.sharpness(bpath)
