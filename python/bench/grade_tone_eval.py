"""智能调色（auto_grade）影调/色彩评估台 —— 真实照片批测。

回答四件事（对应产品目标「更自然 / 层次感更好 / 保留细节 / 想要偏暗」）：
  1. **自然**：亮度均值漂移、白平衡漂移、饱和度漂移、单通道截断增长；
  2. **层次感**：输出灰阶层级利用率、分区斜率、死区（斜率为 0）占比；
  3. **保留细节**：高频能量比、高光/暗部区域内部的层级数与标准差；
  4. **偏暗意图**：输入亮度 → 输出亮度的回归斜率（斜率≈0 即「都被拉成同一个亮度」）。

只测 `autograde`（调色）这一层：不涉及美颜、形变、胶片 LUT、云端。

用法：
    python python/bench/grade_tone_eval.py "<照片目录>"
        [--styles natural,vivid] [--strengths 1.0,0.85]
        [--limit 0] [--out python/bench/out/grade_tone]

产出：
    <out>/report.md        可读报告（总体 + 逐图 + 分区矩阵）
    <out>/per_image.csv    逐图逐档原始读数
    <out>/zones.csv        分区响应（灰阶响应曲线）
    <out>/sbs_<name>.jpg   前后并排对照（仅 --preview 指定的档位）

纪律：只读素材目录，只写 `--out`；不改仓库任何源文件。
"""
from __future__ import annotations

import argparse
import csv
import json
import os
import sys
import time

import numpy as np
from PIL import Image, ImageOps

sys.stdout.reconfigure(encoding="utf-8")

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import autograde  # noqa: E402

# 评估用的长边上限：厂商出片在 12MP 级，2000 长边足以代表统计与像素级形态，
# 同时把批测耗时压到分钟级（查表耗时与像素数成正比）。
_EVAL_LONG = 2000
# 对照图半幅长边
_PREVIEW_LONG = 900

# 影调分区（按输入亮度 0~255）
ZONES = (
    ("深阴影 0-32", 16, 16),
    ("阴影 32-64", 48, 16),
    ("低中间调 64-112", 88, 24),
    ("中调 112-144", 128, 16),
    ("高中间调 144-200", 172, 28),
    ("高光 200-240", 220, 20),
    ("镜面 240-255", 248, 7),
)

# 单通道截断阈值（与 critic.py 一致：任一通道 ≥250 即该处层次不可恢复）
_CLIP = 250
# 死黑阈值
_DEAD = 3


# ---------------------------------------------------------------- 读图与基础量

def load_rgb(path: str, long_edge: int = _EVAL_LONG) -> np.ndarray:
    """EXIF 摆正 → RGB uint8，必要时按长边等比缩小（与 workflows/base.read_image 同口径）。"""
    with Image.open(path) as im:
        img = ImageOps.exif_transpose(im).convert("RGB")
    w, h = img.size
    m = max(w, h)
    if m > long_edge:
        s = long_edge / float(m)
        img = img.resize((max(1, round(w * s)), max(1, round(h * s))), Image.LANCZOS)
    return np.asarray(img, dtype=np.uint8)


def luma_of(arr: np.ndarray) -> np.ndarray:
    x = arr.astype(np.float32) / 255.0
    return x[..., 0] * 0.2126 + x[..., 1] * 0.7152 + x[..., 2] * 0.0722


def chroma_of(arr: np.ndarray) -> np.ndarray:
    """饱和度代理：逐像素 (max-min)/max，0~1。"""
    x = arr.astype(np.float32)
    mx = x.max(axis=-1)
    mn = x.min(axis=-1)
    return np.where(mx > 1e-3, (mx - mn) / np.maximum(mx, 1e-3), 0.0)


def high_freq(arr: np.ndarray) -> float:
    """高频能量：灰度拉普拉斯响应绝对均值（先缩到长边 512，避免大图开销）。"""
    with Image.fromarray(arr) as im:
        w, h = im.size
        m = max(w, h)
        if m > 512:
            s = 512 / float(m)
            im2 = im.resize((max(1, int(w * s)), max(1, int(h * s))), Image.BILINEAR)
            g = np.asarray(im2.convert("L"), dtype=np.float32)
            im2.close()
        else:
            g = np.asarray(im.convert("L"), dtype=np.float32)
    if g.shape[0] < 3 or g.shape[1] < 3:
        return 0.0
    lap = (-4.0 * g[1:-1, 1:-1] + g[:-2, 1:-1] + g[2:, 1:-1]
           + g[1:-1, :-2] + g[1:-1, 2:])
    return float(np.abs(lap).mean())


def metrics(arr: np.ndarray) -> dict:
    """单图读数。全部是与尺寸无关的标量，可直接跨图比较。"""
    img = Image.fromarray(arr)
    lum = luma_of(arr)
    lum8 = np.clip(lum * 255.0 + 0.5, 0, 255).astype(np.uint8)
    ch = chroma_of(arr)
    hl = arr.max(axis=-1) >= _CLIP
    return {
        "mean": round(float(lum.mean()), 4),
        "p5": round(float(np.percentile(lum, 5)), 4),
        "p50": round(float(np.percentile(lum, 50)), 4),
        "p95": round(float(np.percentile(lum, 95)), 4),
        "std": round(float(lum.std()), 4),
        "levels": int(len(np.unique(lum8))),
        "chroma": round(float(ch.mean()), 4),
        "hf": round(high_freq(arr), 3),
        "clipAny": round(float(hl.mean()), 5),
        "dead": round(float((lum * 255 <= _DEAD).mean()), 5),
        "rg": round(float(arr[..., 0].astype(np.float32).mean()
                         / max(1.0, arr[..., 1].astype(np.float32).mean())), 4),
        "bg": round(float(arr[..., 2].astype(np.float32).mean()
                         / max(1.0, arr[..., 1].astype(np.float32).mean())), 4),
        "pixels": int(lum.size),
    }, img


# ---------------------------------------------------------------- 区域级读数

def zone_pixels(in_lum: np.ndarray, out_lum: np.ndarray) -> list[dict]:
    """按**输入**亮度分桶，统计每桶的输入/输出亮度与对比度变化。"""
    rows = []
    for name, center, half in ZONES:
        lo, hi = center - half, center + half
        m = (in_lum * 255 >= lo) & (in_lum * 255 < hi)
        n = int(m.sum())
        if n < 200:
            rows.append({"zone": name, "n": n, "skip": True})
            continue
        i = in_lum[m]
        o = out_lum[m]
        rows.append({
            "zone": name,
            "n": n,
            "in_mean": round(float(i.mean()) * 255, 2),
            "out_mean": round(float(o.mean()) * 255, 2),
            "delta": round((float(o.mean()) - float(i.mean())) * 255, 2),
            # 桶内标准差变化 = 该区域内部层次是否还在
            "in_std": round(float(i.std()) * 255, 2),
            "out_std": round(float(o.std()) * 255, 2),
            "skip": False,
        })
    return rows


def curve_response(stats) -> list[dict]:
    """把该图的调色参数作用在中性灰阶上，得到「输入亮度 → 输出亮度」的分区斜率。

    斜率 < 0.5 = 该区被压平；斜率 = 0 = 该区完全无层次（死区）。
    """
    levels = np.arange(0, 256, dtype=np.float32)
    grey = np.stack([levels, levels, levels], axis=-1) / 255.0
    out = autograde.eval_colors(stats, grey).reshape(-1, 3).mean(axis=1) * 255.0
    slope = np.gradient(out)
    rows = []
    for name, center, _half in ZONES:
        rows.append({
            "zone": name,
            "in": center,
            "out": round(float(out[center]), 2),
            "delta": round(float(out[center]) - center, 2),
            "slope": round(float(slope[center]), 3),
        })
    rows.append({"zone": "【死区统计】", "in": "", "out": "",
                 "delta": f"slope==0 共 {int((slope <= 1e-9).sum())}/256 级",
                 "slope": round(float((slope < 0.5).mean()), 3)})
    return rows


# ---------------------------------------------------------------- 单档处理

def grade_one(arr: np.ndarray, style: str, strength: float) -> tuple[np.ndarray, object]:
    out, stats, _ = autograde.grade(arr, style=style, strength=strength)
    return out, stats


def sbs_image(before: np.ndarray, after: np.ndarray) -> Image.Image:
    """左原图 / 右成片，白线分隔，便于目视对照。"""
    def _fit(a: np.ndarray) -> Image.Image:
        im = Image.fromarray(a)
        w, h = im.size
        m = max(w, h)
        if m > _PREVIEW_LONG:
            s = _PREVIEW_LONG / float(m)
            im = im.resize((max(1, int(w * s)), max(1, int(h * s))), Image.LANCZOS)
        return im

    a, b = _fit(before), _fit(after)
    h = max(a.height, b.height)
    canvas = Image.new("RGB", (a.width + b.width + 8, h), (255, 255, 255))
    canvas.paste(a, (0, (h - a.height) // 2))
    canvas.paste(b, (a.width + 8, (h - b.height) // 2))
    return canvas


# ---------------------------------------------------------------- 报告

def _pct(x: float, digits: int = 2) -> str:
    return f"{x * 100:.{digits}f}%"


def summarize(rows: list[dict]) -> dict:
    """把逐图逐档读数压成一组总体判据。"""
    if not rows:
        return {}
    in_m = np.array([r["m_in"]["mean"] for r in rows])
    out_m = np.array([r["m_out"]["mean"] for r in rows])
    # 输入亮度 → 输出亮度的最小二乘斜率：接近 0 说明「无论多暗都拉成同一亮度」
    slope = float(np.polyfit(in_m, out_m, 1)[0]) if len(in_m) > 1 else float("nan")
    return {
        "n": len(rows),
        "d_mean": round(float(np.mean(out_m - in_m)), 4),
        "abs_d_mean": round(float(np.mean(np.abs(out_m - in_m))), 4),
        "intent_slope": round(slope, 3),
        "in_std": round(float(in_m.std()), 4),
        "out_std": round(float(out_m.std()), 4),
        "levels_ratio": round(float(np.mean([r["m_out"]["levels"] / max(r["m_in"]["levels"], 1)
                                             for r in rows])), 3),
        "hf_ratio": round(float(np.mean([r["m_out"]["hf"] / max(r["m_in"]["hf"], 1e-6)
                                         for r in rows])), 3),
        "clip_delta": round(float(np.mean([r["m_out"]["clipAny"] - r["m_in"]["clipAny"]
                                           for r in rows])) * 100, 4),
        "dead_delta": round(float(np.mean([r["m_out"]["dead"] - r["m_in"]["dead"]
                                           for r in rows])) * 100, 4),
        "chroma_delta": round(float(np.mean([r["m_out"]["chroma"] - r["m_in"]["chroma"]
                                             for r in rows])), 4),
    }


def write_report(path: str, dir_path: str, rows: list[dict], zones: list[dict],
                 groups: dict) -> None:
    L: list[str] = []
    add = L.append
    add("# 智能调色（auto_grade）影调评估")
    add("")
    add(f"- 素材目录：`{dir_path}`")
    add(f"- 生成时间：{time.strftime('%Y-%m-%d %H:%M:%S')}")
    add(f"- 评估口径：长边 {_EVAL_LONG}px；只测 `autograde`（不含胶片 LUT / 美颜 / 形变）")
    add("")

    add("## 1. 总体判据")
    add("")
    add("| 档位 | 图数 | Δ亮度均值 | 绝对漂移 | 意图斜率 | 层级保留 | 高频保留 | 截断Δ | 死黑Δ | 饱和Δ |")
    add("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for key, s in groups.items():
        if not s:
            continue
        add(f"| {key} | {s['n']} | {s['d_mean']:+.4f} | {s['abs_d_mean']:.4f} | "
            f"**{s['intent_slope']}** | {s['levels_ratio']:.3f} | {s['hf_ratio']:.3f} | "
            f"{s['clip_delta']:+.4f}pp | {s['dead_delta']:+.4f}pp | {s['chroma_delta']:+.4f} |")
    add("")
    add("> **意图斜率** = 以输入亮度为自变量、输出亮度为因变量的一元回归斜率。")
    add("> 1.0 = 完全保留原图明暗差异；0 = 无论原图多暗多亮，输出都被拉到同一亮度。")
    add("")

    add("## 2. 逐图读数")
    add("")
    add("| 文件 | 变体 | 档位 | 输入均值 | 输出均值 | Δ | 层级(前→后) | 高频比 | 截断Δ | 暗部Δ |")
    add("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for r in rows:
        add(f"| {r['file']} | {r['variant']} | {r['style']}/{r['strength']} | "
            f"{r['m_in']['mean']:.3f} | "
            f"{r['m_out']['mean']:.3f} | {r['m_out']['mean'] - r['m_in']['mean']:+.3f} | "
            f"{r['m_in']['levels']}→{r['m_out']['levels']} | "
            f"{r['m_out']['hf'] / max(r['m_in']['hf'], 1e-6):.3f} | "
            f"{(r['m_out']['clipAny'] - r['m_in']['clipAny']) * 100:+.3f}pp | "
            f"{(r['m_out']['dead'] - r['m_in']['dead']) * 100:+.3f}pp |")
    add("")

    add("## 3. 分区响应（灰阶曲线：同一组参数作用在中性灰阶上）")
    add("")
    for z in zones:
        add(f"### {z['file']} — {z['style']}/{z['strength']}")
        add("")
        add("| 区域 | 输入 | 输出 | Δ | 斜率 |")
        add("| --- | --- | --- | --- | --- |")
        for row in z["rows"]:
            add(f"| {row['zone']} | {row['in']} | {row['out']} | {row['delta']} | {row['slope']} |")
        add("")
        add(f"- 参数：`{z['stats']}`")
        add("")

    add("## 4. 参数消费情况（风格表里哪些键真正生效）")
    add("")
    used = {"saturation", "vibrance", "warmth"}
    add("| 风格 | 键 | 值 | 是否被读取 |")
    add("| --- | --- | --- | --- |")
    for sname, cfg in autograde.STYLES.items():
        for k, v in cfg.items():
            if k == "name":
                continue
            add(f"| {sname} | `{k}` | {v} | {'是' if k in used else '**否（死参数）**'} |")
    add("")

    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write("\n".join(L) + "\n")


def write_csv(path: str, rows: list[dict]) -> None:
    if not rows:
        return
    keys = ["file", "variant", "style", "strength"]
    keys += [f"in_{k}" for k in rows[0]["m_in"]]
    keys += [f"out_{k}" for k in rows[0]["m_out"]]
    with open(path, "w", encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=keys)
        w.writeheader()
        for r in rows:
            flat = {"file": r["file"], "variant": r["variant"],
                    "style": r["style"], "strength": r["strength"]}
            for k, v in r["m_in"].items():
                flat[f"in_{k}"] = v
            for k, v in r["m_out"].items():
                flat[f"out_{k}"] = v
            w.writerow(flat)


def write_zone_csv(path: str, zones: list[dict]) -> None:
    with open(path, "w", encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["file", "style", "strength", "zone",
                                           "in", "out", "delta", "slope"])
        w.writeheader()
        for z in zones:
            for row in z["rows"]:
                w.writerow({"file": z["file"], "style": z["style"],
                            "strength": z["strength"], **{k: row[k] for k in
                                                          ("zone", "in", "out", "delta", "slope")}})


# ---------------------------------------------------------------- 主流程

def dim(arr: np.ndarray, k: float) -> np.ndarray:
    """模拟「用户原本就想拍暗」：整体乘系数后再量化（等价于减曝光）。"""
    return np.clip(arr.astype(np.float32) * k + 0.5, 0, 255).astype(np.uint8)


def main() -> int:
    ap = argparse.ArgumentParser(description="auto_grade 影调评估台")
    ap.add_argument("dir", help="照片目录")
    ap.add_argument("--styles", default="natural,vivid,warm,cool")
    ap.add_argument("--strengths", default="1.0,0.85")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--out", default=os.path.join(HERE, "out", "grade_tone"))
    ap.add_argument("--preview-style", default="natural")
    ap.add_argument("--preview-strength", type=float, default=0.85)
    ap.add_argument("--preview-all-variants", action="store_true",
                    help="对偏暗变体也出前后对照图（用于目视确认『被提亮』）")
    ap.add_argument("--lowkey", type=float, default=0.0,
                    help="额外跑一组「模拟偏暗」素材：RGB 整体乘该系数（如 0.45 ≈ -1.15EV），"
                         "用于检验『想要的偏暗』是否被自动提亮抹平。0 = 不做")
    args = ap.parse_args()

    src = os.path.abspath(args.dir)
    if not os.path.isdir(src):
        print(f"[错误] 目录不存在：{src}")
        return 2
    styles = [s.strip() for s in args.styles.split(",") if s.strip()]
    strengths = [float(s) for s in args.strengths.split(",") if s.strip()]
    bad = [s for s in styles if s not in autograde.STYLES]
    if bad:
        print(f"[错误] 未知风格：{bad}，可选 {list(autograde.STYLES)}")
        return 2

    files = sorted(p for p in os.listdir(src)
                   if os.path.isfile(os.path.join(src, p))
                   and os.path.splitext(p)[1].lower() in
                   (".jpg", ".jpeg", ".png", ".bmp", ".webp", ".tif", ".tiff"))
    if args.limit > 0:
        files = files[: args.limit]
    if not files:
        print(f"[错误] 目录内没有图片：{src}")
        return 2

    os.makedirs(args.out, exist_ok=True)
    print(f"素材 {len(files)} 张 · 档位 {len(styles) * len(strengths)} · 输出 {args.out}")

    rows: list[dict] = []
    zones: list[dict] = []
    t0 = time.time()
    variants: list[tuple[str, object]] = [("原片", None)]
    if args.lowkey > 0:
        variants.append((f"偏暗×{args.lowkey:g}", args.lowkey))

    for i, name in enumerate(files, 1):
        path = os.path.join(src, name)
        try:
            base = load_rgb(path)
        except Exception as e:  # noqa: BLE001 —— 单张坏图不该中断整批
            print(f"  [{i}/{len(files)}] 跳过 {name}: {e}")
            continue
        print(f"  [{i}/{len(files)}] {name}  {base.shape[1]}x{base.shape[0]}")

        for vlabel, vk in variants:
            before = base if vk is None else dim(base, float(vk))
            m_in, _ = metrics(before)
            print(f"      [{vlabel}] in_mean={m_in['mean']:.3f} levels={m_in['levels']}")

            for style in styles:
                for strength in strengths:
                    ts = time.time()
                    after, stats = grade_one(before, style, strength)
                    m_out, img_out = metrics(after)
                    rows.append({"file": name, "variant": vlabel, "style": style,
                                 "strength": strength, "m_in": m_in, "m_out": m_out})
                    zones.append({"file": f"{name} [{vlabel}]", "style": style,
                                  "strength": strength, "stats": stats.as_meta(),
                                  "rows": curve_response(stats)})
                    print(f"        {style:<6} s={strength:<5} "
                          f"out={m_out['mean']:.3f} d={m_out['mean'] - m_in['mean']:+.3f} "
                          f"levels={m_out['levels']} ({time.time() - ts:.1f}s)")

                    if (style == args.preview_style
                            and abs(strength - args.preview_strength) < 1e-6
                            and (vlabel == "原片" or args.preview_all_variants)):
                        sbs = sbs_image(before, after)
                        stem = os.path.splitext(name)[0][:34]
                        tag = "" if vlabel == "原片" else f"_lowkey{args.lowkey:g}"
                        sbs.save(os.path.join(args.out, f"sbs_{stem}{tag}.jpg"),
                                 "JPEG", quality=90)
                    img_out.close()

    groups = {}
    for vlabel, _ in variants:
        for style in styles:
            for strength in strengths:
                sel = [r for r in rows if r["variant"] == vlabel
                       and r["style"] == style and r["strength"] == strength]
                if sel:
                    groups[f"{vlabel} · {style}/{strength}"] = summarize(sel)

    write_report(os.path.join(args.out, "report.md"), src, rows, zones, groups)
    write_csv(os.path.join(args.out, "per_image.csv"), rows)
    write_zone_csv(os.path.join(args.out, "zones.csv"), zones)
    with open(os.path.join(args.out, "groups.json"), "w", encoding="utf-8") as fh:
        json.dump(groups, fh, ensure_ascii=False, indent=2)

    print(f"\n完成：{len(rows)} 条读数 / {time.time() - t0:.1f}s")
    for k, s in groups.items():
        print(f"  {k:<14} dMean={s['d_mean']:+.3f}  斜率={s['intent_slope']:>5}  "
              f"层级比={s['levels_ratio']:.2f}  高频比={s['hf_ratio']:.2f}")
    print(f"报告：{os.path.join(args.out, 'report.md')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
