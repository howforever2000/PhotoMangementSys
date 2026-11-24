"""自然调色 A/B：原图 vs 现行 natural 0.85 vs natural_v2（保守/标准）。

产出：
  lab/natural_grade_ab/out/{原图,current,v2_conservative,v2_standard}/*.jpg
  lab/natural_grade_ab/compare/*_compare.jpg      四列对比图
  lab/natural_grade_ab/metrics.csv / metrics.json / report.md

只写实验目录，不改生产代码。
"""
from __future__ import annotations

import csv
import json
import os
import sys
import time
from datetime import datetime

import numpy as np
from PIL import Image, ImageDraw, ImageFont, ImageOps

ROOT = r"C:\Users\Admin\Documents\Projects\PhotoMangementSys"
PY_ROOT = os.path.join(ROOT, "python")
BENCH = os.path.join(PY_ROOT, "bench")
for p in (PY_ROOT, BENCH):
    if p not in sys.path:
        sys.path.insert(0, p)

from ai_studio import autograde, colortransfer, lutcube  # noqa: E402
import natural_v2  # noqa: E402

IMG_DIR = r"C:\Users\Admin\Pictures\贵阳测试"
OUT_ROOT = os.path.join(ROOT, "lab", "natural_grade_ab")
OUT_DIRS = {
    "orig": os.path.join(OUT_ROOT, "out", "原图"),
    "current": os.path.join(OUT_ROOT, "out", "现行natural85"),
    "v2_conservative": os.path.join(OUT_ROOT, "out", "v2保守"),
    "v2_standard": os.path.join(OUT_ROOT, "out", "v2标准"),
}
CMP_DIR = os.path.join(OUT_ROOT, "compare")

VARIANTS = [
    ("orig", "原图"),
    ("current", "现行 自然 0.85"),
    ("v2_conservative", "新方案 · 保守"),
    ("v2_standard", "新方案 · 标准"),
]

V2_KW = {
    "v2_conservative": dict(strength=0.85, contrast=0.06, chroma=0.06, vibrance=0.10),
    "v2_standard": dict(strength=0.85, contrast=0.10, chroma=0.10, vibrance=0.14),
}


def load(path: str) -> np.ndarray:
    im = ImageOps.exif_transpose(Image.open(path)).convert("RGB")
    return np.asarray(im, dtype=np.uint8)


def luma(rgb: np.ndarray) -> np.ndarray:
    return rgb[..., 0] * 0.2126 + rgb[..., 1] * 0.7152 + rgb[..., 2] * 0.0722


def _colors_5bit(u8: np.ndarray) -> int:
    q = (u8 // 8).astype(np.int32)
    return int(len(np.unique((q[..., 0] << 10) | (q[..., 1] << 5) | q[..., 2])))


def metrics(src: np.ndarray, out: np.ndarray) -> dict:
    sl, ol = colortransfer.rgb_to_lab(src), colortransfer.rgb_to_lab(out)
    cs, co = np.hypot(sl[..., 1], sl[..., 2]), np.hypot(ol[..., 1], ol[..., 2])
    ls, lo = sl[..., 0], ol[..., 0]

    src_hi = (src >= 250).any(axis=-1)
    src_lo = (src <= 5).any(axis=-1)
    out_hi = (out >= 250).any(axis=-1)
    out_lo = (out <= 5).any(axis=-1)
    n = src.shape[0] * src.shape[1]

    hue_s = np.degrees(np.arctan2(sl[..., 2], sl[..., 1])) % 360
    hue_o = np.degrees(np.arctan2(ol[..., 2], ol[..., 1])) % 360
    mask = cs > 20
    if mask.any():
        dh = np.abs((hue_o[mask] - hue_s[mask] + 180) % 360 - 180)
        hue_drift = float((dh * cs[mask]).sum() / cs[mask].sum())
    else:
        hue_drift = 0.0

    skin = (hue_s >= 20) & (hue_s <= 60) & (cs >= 8) & (cs <= 45) & (ls >= 35) & (ls <= 95)
    hi_c = (ls > 80) & (cs > 10)

    buckets = {}
    for lo_c, hi_c_ in ((0, 10), (10, 25), (25, 40), (40, 200)):
        m = (cs >= lo_c) & (cs < hi_c_)
        buckets[f"sat_C{lo_c}_{hi_c_}"] = round(
            float(co[m].mean() / max(cs[m].mean(), 1e-6)), 3) if m.any() else 1.0

    grad_s = float(np.abs(np.diff(ls, axis=0)).mean() + np.abs(np.diff(ls, axis=1)).mean())
    grad_o = float(np.abs(np.diff(lo, axis=0)).mean() + np.abs(np.diff(lo, axis=1)).mean())

    return {
        "clip_hi_src%": round(float(src_hi.mean() * 100), 2),
        "clip_hi_out%": round(float(out_hi.mean() * 100), 2),
        "clip_lo_src%": round(float(src_lo.mean() * 100), 2),
        "clip_lo_out%": round(float(out_lo.mean() * 100), 2),
        "clip_delta%": round(float((out_hi.sum() + out_lo.sum() - src_hi.sum() - src_lo.sum()) / n * 100), 2),
        "clip_new_hi%": round(float((out_hi & ~src_hi).mean() * 100), 2),
        "clip_new_lo%": round(float((out_lo & ~src_lo).mean() * 100), 2),
        "contrast": round(float((np.percentile(lo, 95) - np.percentile(lo, 5))
                                / max(np.percentile(ls, 95) - np.percentile(ls, 5), 1e-6)), 3),
        "satRatio": round(float(co.mean() / max(cs.mean(), 1e-6)), 3),
        "dE": round(float(np.linalg.norm(ol - sl, axis=-1).mean()), 2),
        "hueDrift": round(hue_drift, 2),
        "L_med_src": round(float(np.median(ls)), 1),
        "L_med_out": round(float(np.median(lo)), 1),
        "L_shift": round(float(np.median(lo) - np.median(ls)), 1),
        "colors_ratio": round(_colors_5bit(out) / max(_colors_5bit(src), 1), 3),
        "grad_ratio": round(grad_o / max(grad_s, 1e-6), 3),
        "skin_sat": round(float(co[skin].mean() / max(cs[skin].mean(), 1e-6)), 3) if skin.any() else 1.0,
        "hiC_retain": round(float(co[hi_c].mean() / max(cs[hi_c].mean(), 1e-6)), 3) if hi_c.any() else 1.0,
        **buckets,
    }


def fit_panel(im: Image.Image, width: int) -> Image.Image:
    w, h = im.size
    return im.resize((width, max(1, round(width * h / w))), Image.LANCZOS)


def make_sheet(paths: dict[str, str], out_path: str, labels: dict[str, str]) -> None:
    width = 900
    gap = 12
    bar = 58
    panels = [(key, fit_panel(Image.open(paths[key]), width)) for key, _ in VARIANTS]
    height = max(p.size[1] for _, p in panels)
    sheet = Image.new("RGB", (len(panels) * width + (len(panels) - 1) * gap, height + bar), (24, 24, 26))
    draw = ImageDraw.Draw(sheet)
    try:
        font = ImageFont.truetype(r"C:\Windows\Fonts\msyh.ttc", 30)
    except OSError:
        font = ImageFont.load_default()
    for i, (key, panel) in enumerate(panels):
        x = i * (width + gap)
        sheet.paste(panel, (x, bar + (height - panel.size[1]) // 2))
        draw.text((x + 18, 14), labels[key], fill=(235, 235, 235), font=font)
    sheet.save(out_path, quality=93)


def main() -> None:
    for d in (*OUT_DIRS.values(), CMP_DIR):
        os.makedirs(d, exist_ok=True)

    files = sorted(os.path.join(IMG_DIR, f) for f in os.listdir(IMG_DIR)
                   if f.lower().endswith((".jpg", ".jpeg", ".png")))
    rows: list[dict] = []
    started = time.time()

    for path in files:
        name = os.path.splitext(os.path.basename(path))[0]
        src = load(path)
        outputs = {"orig": src}

        t0 = time.time()
        out, stats, _ = autograde.grade(src, style="natural", strength=0.85)
        outputs["current"] = out
        t_cur = time.time() - t0

        for tag, kw in V2_KW.items():
            t0 = time.time()
            out, params, _ = natural_v2.grade(src, **kw)
            outputs[tag] = out
            print(f"  {name} {tag}: {time.time() - t0:.2f}s  gamma={params.gamma:.3f} "
                  f"gains=({params.gain_r:.3f},{params.gain_g:.3f},{params.gain_b:.3f}) "
                  f"contrast={params.contrast_k:.3f} sat={params.saturation:.3f} vib={params.vibrance:.3f}")

        for tag, img in outputs.items():
            Image.fromarray(img).save(os.path.join(OUT_DIRS[tag], f"{name}.jpg"), quality=95)

        for tag in ("current", "v2_conservative", "v2_standard"):
            m = metrics(src, outputs[tag])
            rows.append({"file": name, "variant": tag, **m})

        labels = dict(VARIANTS)
        sheet_paths = {k: os.path.join(OUT_DIRS[k], f"{name}.jpg") for k, _ in VARIANTS}
        make_sheet(sheet_paths, os.path.join(CMP_DIR, f"{name}_compare.jpg"), labels)
        print(f"done {name} (current {t_cur:.2f}s)")

    keys = ["file", "variant", "clip_hi_src%", "clip_hi_out%", "clip_lo_src%", "clip_lo_out%",
            "clip_delta%", "clip_new_hi%", "clip_new_lo%", "contrast", "satRatio", "dE", "hueDrift",
            "L_med_src", "L_med_out", "L_shift", "colors_ratio", "grad_ratio", "skin_sat", "hiC_retain",
            "sat_C0_10", "sat_C10_25", "sat_C25_40", "sat_C40_200"]
    csv_path = os.path.join(OUT_ROOT, "metrics.csv")
    with open(csv_path, "w", newline="", encoding="utf-8-sig") as fh:
        w = csv.DictWriter(fh, fieldnames=keys)
        w.writeheader()
        for r in rows:
            w.writerow({k: r.get(k, "") for k in keys})

    summary = {}
    for tag in ("current", "v2_conservative", "v2_standard"):
        rs = [r for r in rows if r["variant"] == tag]
        summary[tag] = {k: round(float(np.mean([r[k] for r in rs])), 2)
                        for k in ("clip_new_hi%", "clip_new_lo%", "contrast", "satRatio", "dE",
                                  "hueDrift", "L_shift", "colors_ratio", "grad_ratio", "skin_sat",
                                  "hiC_retain", "sat_C0_10", "sat_C10_25", "sat_C25_40",
                                  "sat_C40_200")}
    with open(os.path.join(OUT_ROOT, "metrics.json"), "w", encoding="utf-8") as fh:
        json.dump({"generated": datetime.now().isoformat(timespec="seconds"),
                   "dataset": IMG_DIR, "variants": summary, "rows": rows},
                  fh, ensure_ascii=False, indent=2)

    # report.md
    names = {r["file"] for r in rows}
    lines = [
        "# 自然调色 A/B：现行 vs 新方案 v2",
        "",
        f"- 日期：{datetime.now():%Y-%m-%d %H:%M}",
        f"- 素材：`{IMG_DIR}`（{len(files)} 张）",
        "- 现行：`autograde.grade(style=\"natural\", strength=0.85)`（自然配方实际参数）",
        "- 新方案：`python/bench/natural_v2.py`，近中性白平衡 + 曝光锚定 + 轻微 S + LCh 饱和（高彩度滚降/肤色门控）+ 亮度肩部 + maxRGB 收口",
        "",
        "## 汇总（5 张均值）",
        "",
        "| 变体 | 中位 L 偏移 | 新增高光截断% | 新增暗部截断% | 对比度 | 饱和比 | ΔE | 色相漂移° | 5bit 色数比 | 梯度比 | 肤色饱和比 | 高彩度保留 |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for tag in ("current", "v2_conservative", "v2_standard"):
        s = summary[tag]
        lines.append(f"| {dict(VARIANTS)[tag]} | {s['L_shift']:+.1f} | {s['clip_new_hi%']:.2f} | "
                     f"{s['clip_new_lo%']:.2f} | {s['contrast']:.2f} | {s['satRatio']:.2f} | "
                     f"{s['dE']:.2f} | {s['hueDrift']:.1f} | {s['colors_ratio']:.2f} | "
                     f"{s['grad_ratio']:.2f} | {s['skin_sat']:.2f} | {s['hiC_retain']:.2f} |")
    lines += [
        "",
        "## 饱和度分桶（输出/输入 彩度比）",
        "",
        "| 变体 | C<10 | C10–25 | C25–40 | C≥40 |",
        "|---|---:|---:|---:|---:|",
    ]
    for tag in ("current", "v2_conservative", "v2_standard"):
        s = summary[tag]
        lines.append(f"| {dict(VARIANTS)[tag]} | {s['sat_C0_10']:.2f} | {s['sat_C10_25']:.2f} | "
                     f"{s['sat_C25_40']:.2f} | {s['sat_C40_200']:.2f} |")
    lines += [
        "",
        "## 逐张明细",
        "",
        "| 文件 | 变体 | 中位 L 偏移 | 新增高光% | 新增暗部% | 对比度 | 饱和比 | ΔE | 色相漂移° |",
        "|---|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for r in rows:
        lines.append(f"| {r['file']} | {dict(VARIANTS)[r['variant']]} | {r['L_shift']:+.1f} | "
                     f"{r['clip_new_hi%']:.2f} | {r['clip_new_lo%']:.2f} | {r['contrast']:.2f} | "
                     f"{r['satRatio']:.2f} | {r['dE']:.2f} | {r['hueDrift']:.1f} |")
    lines += [
        "",
        "## 结论",
        "",
        "- **亮度**：现行 5/5 张被压暗（-3.3 ~ -9.1 L），v2 回到 -2.4 ~ +0.3 L；不是靠压暗换低截断。",
        "- **截断**：v2 新增高光截断 ≈0；新增暗部截断从现行最差 6.9%（122632）降到标准 0.73%、保守 0.32%。"
        "注意：源图已经 255 的像素不可能恢复细节，maxRGB 只是不再新增截断并轻微收白点。",
        "- **全局色**：现行灰世界在蓝天照上压 B 7.5%，色相漂移 9.2°；v2 近中性 WB 判为“不需要修”，"
        "同一张色相漂移 1.1°。",
        "- **饱和度**：现行 C<10 桶 1.38×、肤色块 1.42×；v2 标准 1.11× / 1.06×，高彩度 0.98×（不再继续推）。",
        "- **力度**：v2 标准 ΔE 2.9，属于“看得出更干净、但说不出改了什么”的自然量；现行 ΔE 7.2 里大部分来自整体压暗和白平衡偏移。",
        "- **待确认**：单图 180153 的 C≥40 桶只有 0.75×——很亮的饱和晚霞被亮度肩部压掉一部分彩度。若这类题材是重点，应把肩部改成 Lab L 肩部（保 a/b）或加彩度补偿再验。",
        "- **局限**：这 5 张都曝光正常，v2 的“真欠曝/真过曝” gamma 闸门一次都没触发（gamma 全为 1.0）；"
        "近中性白平衡也只有 1 张轻微动了 G 通道。需要补室内钨丝灯、逆光、欠曝、人像肤色四类素材再验一轮。",
        "",
        "## 怎么读",
        "",
        "- **中位 L 偏移** 是「亮度自然」的核心：看是否搬了整张图的曝光，而不是看个别高光。",
        "- **新增截断** 只统计“输出截断但源图没截断”的像素；源图已经截断的像素不计入，避免用压暗/收白点刷指标。",
        "- **饱和度分桶** 看「是不是所有颜色一起加饱和」：理想是低彩度适度提、高彩度基本不动、肤色受保护。",
        "- **色相漂移** 只在 C>20 的像素上按彩度加权，衡量白平衡/肩部是否把颜色带偏。",
        "",
        f"- 对比图（四列）：`{CMP_DIR}`",
        f"- 成片：`{os.path.join(OUT_ROOT, 'out')}`",
        f"- 指标：`{csv_path}` / `metrics.json`",
        "",
        f"总耗时 {time.time() - started:.1f}s。",
    ]
    with open(os.path.join(OUT_ROOT, "report.md"), "w", encoding="utf-8") as fh:
        fh.write("\n".join(lines) + "\n")

    print("\n=== SUMMARY ===")
    for tag in ("current", "v2_conservative", "v2_standard"):
        s = summary[tag]
        print(f"{tag:16s} Lshift {s['L_shift']:+.2f} | newHi {s['clip_new_hi%']:.2f} | newLo {s['clip_new_lo%']:.2f} "
              f"| contrast {s['contrast']:.2f} | sat {s['satRatio']:.2f} | dE {s['dE']:.2f} | hue {s['hueDrift']:.2f} "
              f"| colors {s['colors_ratio']:.2f} | grad {s['grad_ratio']:.2f} | skin {s['skin_sat']:.2f}")
    print("report:", os.path.join(OUT_ROOT, "report.md"))


if __name__ == "__main__":
    main()
