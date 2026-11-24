"""参考图调色三层解耦·基准（FEAT-219）。

对指定素材目录与参考组跑 `refgrade.transfer`，输出指标 JSON 与对比拼图。
指标口径见 `docs/FEAT-219-参考图调色三层解耦.md` 附录 A；本脚本是那套公式的
可执行版本，不依赖会话里的实验脚本。

用法（PowerShell）：
  .venv-proto\\Scripts\\python.exe python\\bench\\grade_ref_eval.py `
    --material-dir "C:\\Users\\Admin\\Pictures\\贵阳测试" `
    --group "清橙胶片=C:\\Users\\Admin\\Pictures\\清橙胶片" `
    --group "日系=C:\\Users\\Admin\\Pictures\\日系" `
    --out lab\\colortransfer\\feat219 --limit-materials 2 --limit-refs 3
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont, ImageOps

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))

from ai_studio import colortransfer, refgrade  # noqa: E402

_IMG_EXT = (".jpg", ".jpeg", ".png", ".webp", ".bmp")
_DIRS = np.random.default_rng(0).normal(size=(12, 3))
_DIRS /= np.linalg.norm(_DIRS, axis=1, keepdims=True)


def _read(path: str) -> np.ndarray:
    with Image.open(path) as img:
        return np.asarray(ImageOps.exif_transpose(img).convert("RGB"), dtype=np.uint8)


def _read_ref(path: str, long_edge: int = 1024) -> np.ndarray:
    """参考图按生产口径降到长边 1024（统计量与分辨率无关，全量转 Lab 太慢）。"""
    img = Image.open(path)
    img = ImageOps.exif_transpose(img).convert("RGB")
    w, h = img.size
    m = max(w, h)
    if m > long_edge:
        s = long_edge / float(m)
        img = img.resize((max(1, int(round(w * s))), max(1, int(round(h * s)))), Image.LANCZOS)
    return np.asarray(img, dtype=np.uint8)


def _clip_pct(arr: np.ndarray) -> float:
    return float(np.mean(np.any((arr >= 250) | (arr <= 5), axis=-1)) * 100.0)


def _hue_drift_from_lab(s: np.ndarray, o: np.ndarray) -> float:
    c = np.hypot(s[..., 1], s[..., 2])
    mask = c > 20
    if not mask.any():
        return 0.0
    hs = np.degrees(np.arctan2(s[..., 2], s[..., 1]))
    ho = np.degrees(np.arctan2(o[..., 2], o[..., 1]))
    d = np.abs((ho - hs + 180.0) % 360.0 - 180.0)
    return float(np.average(d[mask], weights=c[mask]))


def _contrast_from_lab(s: np.ndarray, o: np.ndarray) -> float:
    span_s = np.percentile(s[..., 0], 95) - np.percentile(s[..., 0], 5)
    span_o = np.percentile(o[..., 0], 95) - np.percentile(o[..., 0], 5)
    return float(span_o / max(span_s, 1e-6))


def _swd_from_lab(s: np.ndarray, o: np.ndarray) -> float:
    s = s / np.array([100.0, 60.0, 60.0], dtype=np.float32)
    o = o / np.array([100.0, 60.0, 60.0], dtype=np.float32)
    vals = []
    for d in _DIRS:
        ps = np.percentile(s @ d, np.linspace(0, 100, 64))
        po = np.percentile(o @ d, np.linspace(0, 100, 64))
        vals.append(float(np.mean(np.abs(ps - po))))
    return float(np.mean(vals))


def _luma(arr: np.ndarray) -> np.ndarray:
    return (0.2126 * arr[..., 0] + 0.7152 * arr[..., 1] + 0.0722 * arr[..., 2]) / 255.0


def _grad_ratio(src: np.ndarray, out: np.ndarray) -> float:
    gs = np.abs(np.diff(_luma(src), axis=0)).mean() + np.abs(np.diff(_luma(src), axis=1)).mean()
    go = np.abs(np.diff(_luma(out), axis=0)).mean() + np.abs(np.diff(_luma(out), axis=1)).mean()
    return float(go / max(gs, 1e-6))


def _colors_ratio(src: np.ndarray, out: np.ndarray) -> float:
    def uniq(a: np.ndarray) -> int:
        q = ((a.astype(np.int32) * 31 + 127) // 255)
        return len(np.unique(q.reshape(-1, 3), axis=0))
    return float(uniq(out) / max(uniq(src), 1))


def _highlight_chroma(src: np.ndarray, src_lab: np.ndarray, out_lab: np.ndarray) -> float:
    mask = _luma(src) > 0.80
    if not mask.any():
        return 1.0
    cs = np.hypot(src_lab[..., 1], src_lab[..., 2])[mask].mean()
    co = np.hypot(out_lab[..., 1], out_lab[..., 2])[mask].mean()
    return float(co / max(cs, 1e-6))


def _metrics(src: np.ndarray, out: np.ndarray) -> dict:
    s = colortransfer.rgb_to_lab(src)
    o = colortransfer.rgb_to_lab(out)
    cs = np.hypot(s[..., 1], s[..., 2]).mean()
    co = np.hypot(o[..., 1], o[..., 2]).mean()
    return {
        "clip": round(_clip_pct(out), 3),
        "clip_src": round(_clip_pct(src), 3),
        "clip_delta": round(_clip_pct(out) - _clip_pct(src), 3),
        "hueDrift": round(_hue_drift_from_lab(s, o), 2),
        "satRatio": round(float(co / max(cs, 1e-6)), 3),
        "contrast": round(_contrast_from_lab(s, o), 3),
        "dE": round(float(np.linalg.norm(o - s, axis=-1).mean()), 2),
        "swd": round(_swd_from_lab(s, o), 3),
        "grad": round(_grad_ratio(src, out), 3),
        "colors": round(_colors_ratio(src, out), 3),
        "hiChroma": round(_highlight_chroma(src, s, o), 3),
    }


def _thumb(path: str, width: int = 360) -> Image.Image:
    img = Image.open(path).convert("RGB")
    h = max(1, int(round(img.height * width / max(img.width, 1))))
    return img.resize((width, h), Image.LANCZOS)


def _font(size: int = 20):
    for p in (r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\simhei.ttf"):
        if os.path.isfile(p):
            return ImageFont.truetype(p, size)
    return ImageFont.load_default()


def _sheet(paths: list[str], labels: list[str], out_path: str) -> None:
    thumbs = [_thumb(p) for p in paths]
    cw, gap, top = 360, 10, 46
    row_h = max(t.height for t in thumbs) + 34
    W = gap + len(thumbs) * (cw + gap)
    H = top + row_h
    sheet = Image.new("RGB", (W, H), (245, 245, 247))
    draw = ImageDraw.Draw(sheet)
    font = _font(20)
    for i, t in enumerate(thumbs):
        x = gap + i * (cw + gap)
        draw.text((x, 12), labels[i], fill=(20, 20, 20), font=font)
        sheet.paste(t, (x, top))
    sheet.save(out_path, "JPEG", quality=92)


def _pick(ref_dir: str, limit: int) -> list[str]:
    files = sorted(p for p in Path(ref_dir).iterdir() if p.suffix.lower() in _IMG_EXT)
    return [str(p) for p in files[:limit]]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--material-dir", required=True)
    ap.add_argument("--group", action="append", required=True, help="名称=参考图目录，可重复")
    ap.add_argument("--out", default=str(ROOT / "lab" / "colortransfer" / "feat219"))
    ap.add_argument("--limit-materials", type=int, default=2)
    ap.add_argument("--limit-refs", type=int, default=3)
    ap.add_argument("--strength", type=float, default=1.0)
    ap.add_argument("--tone-weight", type=float, default=0.85)
    ap.add_argument("--chroma-weight", type=float, default=1.0)
    ap.add_argument("--shoulders", default="ratio,channel")
    args = ap.parse_args()

    out_root = Path(args.out)
    out_root.mkdir(parents=True, exist_ok=True)
    materials = sorted(p for p in Path(args.material_dir).iterdir() if p.suffix.lower() in _IMG_EXT)
    materials = materials[: args.limit_materials]
    if not materials:
        print("没有素材图")
        return 2

    groups = []
    for spec in args.group:
        name, _, ref_dir = spec.partition("=")
        if not ref_dir:
            raise SystemExit(f"--group 需要 名称=目录 的形式: {spec}")
        groups.append((name.strip(), ref_dir.strip()))

    all_metrics: dict = {"materials": [p.name for p in materials], "groups": {}}
    for name, ref_dir in groups:
        ref_paths = _pick(ref_dir, args.limit_refs)
        if not ref_paths:
            print(f"[{name}] 没有参考图，跳过")
            continue
        refs = [_read_ref(p) for p in ref_paths]
        t0 = time.perf_counter()
        profile = refgrade.build_profile(refs)
        t_profile = time.perf_counter() - t0
        print(f"\n== {name} == 参考 {len(refs)} 张，kept={profile.kept} dropped={profile.dropped}，profile {t_profile*1000:.0f}ms")

        gdir = out_root / name
        gdir.mkdir(parents=True, exist_ok=True)
        rows = []
        for mpath in materials:
            src = _read(str(mpath))
            stem = mpath.stem
            row = {"material": mpath.name, "methods": {}}
            variants: dict[str, np.ndarray] = {}
            merged = colortransfer.merge_reference_stats([colortransfer.reference_stats(r) for r in refs])
            variants["现有25"] = colortransfer.reinhard_transfer_stats(
                src, merged, l_weight=1.0, strength=0.25,
            )
            variants["现有100"] = colortransfer.reinhard_transfer_stats(
                src, merged, l_weight=1.0, strength=1.0,
            )
            for shoulder in [s.strip() for s in args.shoulders.split(",") if s.strip()]:
                t0 = time.perf_counter()
                out, lut, n = refgrade.transfer(
                    src, profile, strength=args.strength, tone_weight=args.tone_weight,
                    chroma_weight=args.chroma_weight, keep_exposure=True, protect_skin=False,
                    shoulder_mode=shoulder,
                )
                ms = (time.perf_counter() - t0) * 1000
                key = f"三层-{shoulder}"
                variants[key] = out
                mt = {**_metrics(src, out), "ms": round(ms)}
                row["methods"][key] = mt
                print(f"  {stem} {key}: clip {mt['clip']:.2f}%  con {mt['contrast']:.3f}  "
                      f"dE {mt['dE']:.1f}  hiC {mt['hiChroma']:.3f}  {ms:.0f}ms")
            for key, out in variants.items():
                if key not in row["methods"]:
                    row["methods"][key] = _metrics(src, out)
                p = gdir / f"{stem}_{key}.jpg"
                Image.fromarray(out).save(p, "JPEG", quality=95)
            # 对比图：原图 / 现有25 / 现有100 / 三层各肩部
            labels = ["原图", "现有25", "现有100"] + [f"三层-{s}" for s in [x.strip() for x in args.shoulders.split(",") if x.strip()]]
            paths = [str(mpath)] + [str(gdir / f"{stem}_{k}.jpg") for k in ["现有25", "现有100"] + [f"三层-{s}" for s in [x.strip() for x in args.shoulders.split(",") if x.strip()]]]
            _sheet(paths, labels, str(gdir / f"{stem}_对比.jpg"))
            rows.append(row)

        all_metrics["groups"][name] = {
            "refs": [Path(p).name for p in ref_paths],
            "kept": list(profile.kept),
            "dropped": list(profile.dropped),
            "profile_ms": round(t_profile * 1000),
            "rows": rows,
        }

    (out_root / "metrics.json").write_text(json.dumps(all_metrics, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"\n指标与对比图写入 {out_root}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
