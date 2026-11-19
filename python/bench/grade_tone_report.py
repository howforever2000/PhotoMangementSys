"""智能调色三列对比 HTML 报告：原图 / 现有方案 / 新方案。

职责：跑两套调色算法、落三组图、拼成单文件可交互 HTML。
算法：现有方案 = `ai_studio.autograde`（v0），新方案 = `grade_tone_v2`（v2 原型）。
度量：全部复用 `grade_tone_eval`，本模块只做「取数 → 落图 → 拼页面」。

产出结构：
    <out>/index.html
    <out>/src/<key>.jpg      原图（含偏暗变体）
    <out>/v0/<key>.jpg       现有方案出片
    <out>/v2/<key>.jpg       新方案出片

用法：
    python python/bench/grade_tone_report.py "<照片目录>"
        [--styles natural,vivid] [--strengths 0.85] [--lowkey 0.45] [--limit 0]
        [--out python/bench/out/grade_tone_html]
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import time

import numpy as np
from PIL import Image

sys.stdout.reconfigure(encoding="utf-8")

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.dirname(HERE))

import grade_tone_v2 as v2  # noqa: E402
from ai_studio import autograde as v0  # noqa: E402
from grade_tone_eval import dim, load_rgb, metrics  # noqa: E402

# 三列并排时每格宽度约 1/3 视口，720 长边足够看清细节，整包体积可控。
_PREVIEW_LONG = 720
_CURVE_SAMPLES = 96
_IMG_EXT = (".jpg", ".jpeg", ".png", ".bmp", ".webp", ".tif", ".tiff")

# 两套算法的统一入口，便于 report 层不关心实现差异。
ENGINES = (
    ("v0", "现有方案", lambda a, s, st: v0.grade(a, style=s, strength=st)),
    ("v2", "新方案", lambda a, s, st: v2.grade(a, style=s, strength=st)),
)


def fit(img: np.ndarray, long_edge: int = _PREVIEW_LONG) -> Image.Image:
    """等比缩到长边上限（只缩不放）。"""
    im = Image.fromarray(img)
    w, h = im.size
    m = max(w, h)
    if m > long_edge:
        s = long_edge / float(m)
        im = im.resize((max(1, round(w * s)), max(1, round(h * s))), Image.LANCZOS)
    return im


def curve_of(engine: str, stats) -> dict:
    """把该组参数作用在中性灰阶上，得到可绘制的 (输入, 输出) 曲线。

    两套算法的参数对象不同（v0 是 GradeStats、v2 是 GradeV2），但都有
    `eval_colors`，所以这里按引擎分派，而不是在页面层判断。

    除曲线外还给出三个只在灰阶上才能量出来的量：
      - `dead`：斜率=0 的占比（层次被焊死的灰阶比例）；
      - `span`：输出动态范围；
      - `top5`：输入 250~255 这 5 级在输出里还剩几级（**高光细节保留率**）。
        为什么必须单独给这个：`clipAny` 只统计 ≥250 的像素，两套方案都把高光压到
        250 以下时它会双双归零，完全看不出差别；而 `top5` 能看出「压下去之后
        那 5 级还剩下多少区分度」—— 实测现行方案均值仅 35%，新方案 ~75%。
    """
    ev = v0.eval_colors if engine == "v0" else v2.eval_colors
    levels = np.linspace(0.0, 1.0, _CURVE_SAMPLES)
    grey = np.stack([levels, levels, levels], axis=-1)[None]
    out = ev(stats, grey).reshape(-1, 3).mean(axis=1)
    pts = [[round(float(a) * 255.0, 1), round(float(b) * 255.0, 1)]
           for a, b in zip(levels, out)]
    dead = float((np.diff(out) <= 1e-6).mean())

    top = np.linspace(250.0 / 255.0, 1.0, 6)[None]
    top_grey = np.stack([top, top, top], axis=-1)
    top_out = ev(stats, top_grey).reshape(-1, 3).mean(axis=1) * 255.0
    top_span = float(top_out.max() - top_out.min())

    return {"pts": pts, "dead": round(dead, 3),
            "span": round(float(out[-1] - out[0]) * 255.0, 1),
            "top5": round(top_span / 5.0, 3)}


def collect(src: str, files: list[str], styles: list[str], strengths: list[float],
            lowkey: float, out_dir: str) -> list[dict]:
    """逐图逐档跑两套算法，落「原图 / v0 / v2」三组缩略图并收集读数。"""
    dirs = {k: os.path.join(out_dir, k) for k in ("src", "v0", "v2")}
    for d in dirs.values():
        os.makedirs(d, exist_ok=True)

    variants: list[tuple[str, float | None]] = [("原片", None)]
    if lowkey > 0:
        variants.append((f"偏暗×{lowkey:g}", lowkey))

    items: list[dict] = []
    for i, name in enumerate(files, 1):
        path = os.path.join(src, name)
        try:
            base = load_rgb(path, long_edge=_PREVIEW_LONG)
        except Exception as e:  # noqa: BLE001 —— 单张坏图不该中断整批
            print(f"  [{i}/{len(files)}] 跳过 {name}: {e}")
            continue

        for vlabel, vk in variants:
            before = base if vk is None else dim(base, float(vk))
            m_in, _ = metrics(before)
            skey = f"{i:02d}_{'orig' if vk is None else 'dark'}"
            fit(before).save(os.path.join(dirs["src"], f"{skey}.jpg"), "JPEG", quality=88)
            print(f"  [{i}/{len(files)}] {name} [{vlabel}] in={m_in['mean']:.3f}")

            for style in styles:
                for strength in strengths:
                    runs = []
                    for eng, _, fn in ENGINES:
                        out, stats, _lut = fn(before, style, strength)
                        m_out, _ = metrics(out)
                        ekey = f"{skey}_{style}_{strength:g}"
                        fit(out).save(os.path.join(dirs[eng], f"{ekey}.jpg"),
                                      "JPEG", quality=88)
                        runs.append({
                            "eng": eng,
                            "m": m_out,
                            "curve": curve_of(eng, stats),
                            "stats": stats.as_meta(),
                        })
                    items.append({
                        "file": name, "variant": vlabel,
                        "skey": skey, "ekey": f"{skey}_{style}_{strength:g}",
                        "style": style, "strength": strength,
                        "mIn": m_in, "runs": runs,
                    })
    return items


def summarize(items: list[dict], eng: str) -> dict:
    """某个方案在当前筛选集上的总体判据（含意图斜率）。"""
    if not items:
        return {}
    x = np.array([it["mIn"]["mean"] for it in items])
    sel = [next(r for r in it["runs"] if r["eng"] == eng) for it in items]
    y = np.array([r["m"]["mean"] for r in sel])
    slope = float(np.polyfit(x, y, 1)[0]) if np.ptp(x) > 1e-9 else float("nan")
    return {
        "dMean": round(float(np.mean(y - x)), 4),
        "absD": round(float(np.mean(np.abs(y - x))), 4),
        "slope": round(slope, 3),
        "lvR": round(float(np.mean([r["m"]["levels"] / max(it["mIn"]["levels"], 1)
                                    for it, r in zip(items, sel)])), 3),
        "hfR": round(float(np.mean([r["m"]["hf"] / max(it["mIn"]["hf"], 1e-6)
                                    for it, r in zip(items, sel)])), 3),
        "clipD": round(float(np.mean([r["m"]["clipAny"] - it["mIn"]["clipAny"]
                                      for it, r in zip(items, sel)])) * 100, 3),
        "deadD": round(float(np.mean([r["m"]["dead"] - it["mIn"]["dead"]
                                      for it, r in zip(items, sel)])) * 100, 3),
        "top5": round(float(np.mean([r["curve"]["top5"] for r in sel])), 3),
    }


# ---------------------------------------------------------------- 页面模板

_PAGE = r"""<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>智能调色对比 · 原图 / 现有方案 / 新方案</title>
<style>
:root{
  --bg:#0d1014; --panel:#151a21; --panel2:#1b222b; --line:#2a333f;
  --fg:#e8edf4; --dim:#8b98a9; --accent:#5aa9ff; --ok:#3ecf8e; --warn:#f5b942; --bad:#ff6b6b;
  --old:#c98b4b; --new:#5aa9ff;
}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);
  font:14px/1.6 "Segoe UI","Microsoft YaHei",system-ui,sans-serif}
header{padding:22px 26px 14px;border-bottom:1px solid var(--line);background:var(--panel)}
h1{margin:0 0 6px;font-size:19px;font-weight:600;letter-spacing:.3px}
.sub{color:var(--dim);font-size:12.5px}
.sub code{background:#000;padding:1px 5px;border-radius:4px;color:#9fd0ff}
.bar{position:sticky;top:0;z-index:20;display:flex;flex-wrap:wrap;gap:10px 16px;
  align-items:center;padding:12px 26px;background:rgba(21,26,33,.96);
  border-bottom:1px solid var(--line);backdrop-filter:blur(8px)}
.bar label{color:var(--dim);font-size:12px;margin-right:4px}
.bar select{background:var(--panel2);color:var(--fg);border:1px solid var(--line);
  border-radius:6px;padding:5px 8px;font-size:13px;outline:none}
.bar select:focus{border-color:var(--accent)}
.seg{display:inline-flex;border:1px solid var(--line);border-radius:7px;overflow:hidden}
.seg button{background:var(--panel2);color:var(--dim);border:0;padding:5px 12px;
  font-size:12.5px;cursor:pointer}
.seg button.on{background:var(--accent);color:#04121f;font-weight:600}
.count{color:var(--dim);font-size:12px;margin-left:auto}

.summary{padding:18px 26px;border-bottom:1px solid var(--line)}
table.kpi{border-collapse:collapse;width:100%;max-width:860px;font-size:13px}
table.kpi th,table.kpi td{padding:7px 12px;border-bottom:1px solid var(--line);
  text-align:right;font-variant-numeric:tabular-nums}
table.kpi th:first-child,table.kpi td:first-child{text-align:left;color:var(--dim);
  font-weight:400;font-size:12.5px}
table.kpi thead th{color:var(--dim);font-weight:500;font-size:12px}
table.kpi thead th.old{color:var(--old)} table.kpi thead th.new{color:var(--new)}
td.ok{color:var(--ok)} td.warn{color:var(--warn)} td.bad{color:var(--bad)}
td.dim{color:#4e5a69}
.note-inline{color:var(--dim);font-size:11.5px;margin-top:8px}

.grid{display:flex;flex-direction:column;gap:20px;padding:20px 26px 60px}
.card{background:var(--panel);border:1px solid var(--line);border-radius:12px;overflow:hidden}
.head{display:flex;align-items:baseline;gap:8px;padding:11px 14px 9px;flex-wrap:wrap}
.name{font-size:13.5px;font-weight:600}
.tag{font-size:11px;padding:1px 7px;border-radius:20px;border:1px solid var(--line);
  color:var(--dim);white-space:nowrap}
.tag.dark{color:var(--warn);border-color:#5a4620}
.tag.grade{color:var(--accent);border-color:#22405e}

/* 三列固定对比：原图 / 现有方案 / 新方案 */
.trio{display:grid;grid-template-columns:1fr 1fr 1fr;gap:1px;background:var(--line)}
.col{background:#000;position:relative;line-height:0}
.col img{display:block;width:100%;height:auto}
.col .cap{position:absolute;top:0;left:0;right:0;display:flex;justify-content:space-between;
  align-items:center;padding:6px 9px;font-size:11.5px;letter-spacing:.4px;
  background:linear-gradient(180deg,rgba(0,0,0,.78),rgba(0,0,0,0));line-height:1.4}
.col .cap b{font-weight:600}
.col.c-src .cap b{color:#dfe7f0}
.col.c-old .cap b{color:var(--old)}
.col.c-new .cap b{color:var(--new)}
.col .delta{font-size:11px;font-variant-numeric:tabular-nums}
.col .delta.ok{color:var(--ok)} .col .delta.bad{color:var(--bad)} .col .delta.warn{color:var(--warn)}

.readout{display:grid;grid-template-columns:1fr 1fr 1fr;gap:1px;background:var(--line);
  border-top:1px solid var(--line)}
.rd{background:var(--panel);padding:9px 12px;font-size:12px}
.rd .hd{font-size:11px;color:var(--dim);margin-bottom:3px}
.rd .line{color:var(--dim);font-variant-numeric:tabular-nums}
.rd .line b{color:var(--fg)}
.rd .line b.ok{color:var(--ok)} .rd .line b.warn{color:var(--warn)} .rd .line b.bad{color:var(--bad)}

.curves{display:grid;grid-template-columns:1fr 1fr;gap:14px;padding:10px 14px 14px}
.curves .cbox{background:var(--panel2);border:1px solid var(--line);border-radius:8px;
  padding:7px 9px}
.curves .ct{font-size:11px;color:var(--dim);margin-bottom:3px}
.curves svg{width:100%;height:88px;display:block}
.curves .cf{font-size:10.5px;color:var(--dim);margin-top:2px}

.note{padding:0 26px 30px;color:var(--dim);font-size:12px}
.note h3{color:var(--fg);font-size:13.5px;margin:22px 0 6px}
.note ul{margin:6px 0;padding-left:20px} .note li{margin:4px 0}
.note code{background:#000;padding:1px 5px;border-radius:4px;color:#9fd0ff;font-size:11.5px}
</style>
</head>
<body>
<header>
  <h1>智能调色三列对比 · 原图 / 现有方案 / 新方案</h1>
  <div class="sub">
    素材目录 <code>__SRC__</code> · 生成于 __TIME__ · 共 __NIMG__ 张 / __NITEM__ 组<br>
    <b>左＝原图</b>，<b>中＝现有方案</b>（<code>ai_studio.autograde</code>），
    <b>右＝新方案</b>（<code>grade_tone_v2</code> 原型）。只测调色这一层：不含胶片 LUT、美颜、形变。
  </div>
</header>

<div class="bar">
  <div class="seg" id="segVar">
    <button data-var="原片" class="on">原片</button>
    <button data-var="__DARKLABEL__">偏暗模拟</button>
    <button data-var="全部">全部变体</button>
  </div>
  <div><label>风格</label><select id="selStyle">__STYLEOPTS__</select></div>
  <div><label>力度</label><select id="selStrength">__STRENGTHOPTS__</select></div>
  <span class="count" id="count"></span>
</div>

<div class="summary">
  <table class="kpi" id="kpi"></table>
  <div class="note-inline" id="kpiNote"></div>
</div>

<div class="grid" id="grid"></div>

<div class="note">
  <h3>新方案改了什么（每条都对应一处实测问题）</h3>
  <ul>
    <li><b>去掉「均值收敛到 0.40~0.50」</b>：现行实现无条件把中位数拉进这条带子，
      于是偏暗素材被抬 +0.25。新方案只在<b>真的死黑/真的截断</b>时介入，
      且目标值本身也封顶 —— 这是「想要的偏暗」能保住的原因。</li>
    <li><b>不做黑白场归一化</b>：低 key 画面本就不该占满 0~1。现行白点拉伸会把单通道推爆
      （实测截断 0.008%→22.6%），新方案锚点固定 <code>(0,0)</code> 与 <code>(1,1)</code>，
      输出恒在界内。</li>
    <li><b>分区 S 曲线替代 gamma 提亮</b>：中调锚点钉死（保均值＝保意图），
      暗部微压、亮部微抬，层次来自<b>斜率变化</b>而不是整体提亮。曲线用 Fritsch–Carlson
      单调三次插值，保证不会在暗部造出反向台阶。</li>
    <li><b>只压一次肩部</b>：现行实现在白平衡 / gamma / 色温 / 收口各压一次（实测 2~3 次），
      纯白 255 被压到 235~244。新方案的白平衡与色温增益都归一化到「最大通道 = 1.0」
      （只压不提），通道永不过界，因此只在<b>确实</b>有像素溢出时压一次。</li>
    <li><b>白平衡只用近中性像素</b>：现行灰世界把草地/蓝天当色偏修掉（实测 6/10 张顶到
      ±0.10 上限、系统性暖化 +0.03~0.075、蓝紫色相偏 −5~−8°）。新方案只采信低饱和像素，
      上限 ±0.05。</li>
    <li><b><code>contrast</code> 这次真的接进曲线</b>：现行 <code>STYLES</code> 里的
      <code>contrast</code> 与 <code>wb</code> 从未被读取，是死参数 —— 所以「鲜明」相对
      「自然」只有饱和度差异，没有层次差异。</li>
  </ul>

  <h3>怎么读每张卡</h3>
  <ul>
    <li>三张图下方各有读数：<b>Δ均值</b>（相对原图）、<b>层级</b>（用到的亮度级数）、
      <b>高频</b>（细节量）、<b>截断</b>（高光丢没丢）、<b>死黑</b>（暗部压没压死）。</li>
    <li>再下方是两条<b>灰阶响应曲线</b>：横轴输入 0–255、纵轴输出 0–255。
      <b>水平的那一段＝层次被焊死</b>；斜率 &gt; 2 的那段＝把噪声一起放大。</li>
    <li>顶部表格的 <b>意图斜率</b>：输入亮度 → 输出亮度的回归斜率。
      1.0＝多暗就还多暗；<b>接近 0＝无论多暗都拉成同一亮度</b>。</li>
  </ul>
</div>

<script>
const ITEMS = __DATA__;
const SRC = __SRCJSON__;
// 默认视图：只显示「原片 × 默认风格 × 默认力度」——一张图一组，不铺开矩阵。
// 其余档位靠顶栏筛选主动选择，避免首屏把同一张图重复几十遍。
const DEFAULT_STYLE = __DEFSTYLE__;
const DEFAULT_STRENGTH = __DEFSTR__;
const state = { variant: "原片", style: DEFAULT_STYLE, strength: String(DEFAULT_STRENGTH) };

function clsGoodHi(v, good, warn){ return v >= good ? "ok" : (v >= warn ? "warn" : "bad"); }

function filtered(){
  return ITEMS.filter(it =>
    (state.variant === "全部" || it.variant === state.variant) &&
    (state.style === "全部" || it.style === state.style) &&
    (state.strength === "全部" || String(it.strength) === state.strength));
}

/* 首屏别把同一张图铺成几十张卡：按文件去重，只留当前筛选下每个文件的第一条。
   用户主动把风格/力度切到「全部」时才展开矩阵 —— 那时他明确想看对比。 */
function dedupe(list){
  const expand = (state.style === "全部" && state.strength === "全部");
  if(expand) return list;
  const seen = new Set();
  return list.filter(it => {
    if(seen.has(it.file)) return false;
    seen.add(it.file); return true;
  });
}

function run(it, eng){ return it.runs.find(r => r.eng === eng); }

function summaryOf(list, eng){
  if(!list.length) return null;
  const x = list.map(i => i.mIn.mean);
  const rs = list.map(i => run(i, eng));
  const y = rs.map(r => r.m.mean);
  const n = x.length;
  const mx = x.reduce((a,b)=>a+b,0)/n, my = y.reduce((a,b)=>a+b,0)/n;
  let num=0, den=0;
  for(let i=0;i<n;i++){ num+=(x[i]-mx)*(y[i]-my); den+=(x[i]-mx)*(x[i]-mx); }
  const slope = den>1e-9 ? num/den : NaN;
  const d = y.map((v,i)=>v-x[i]);
  return {
    dMean: d.reduce((a,b)=>a+b,0)/n,
    absD: d.map(Math.abs).reduce((a,b)=>a+b,0)/n,
    slope,
    lvR: rs.reduce((a,r,i)=>a + r.m.levels/Math.max(list[i].mIn.levels,1),0)/n,
    hfR: rs.reduce((a,r,i)=>a + r.m.hf/Math.max(list[i].mIn.hf,1e-6),0)/n,
    clipD: rs.reduce((a,r,i)=>a + (r.m.clipAny-list[i].mIn.clipAny)*100,0)/n,
    deadD: rs.reduce((a,r,i)=>a + (r.m.dead-list[i].mIn.dead)*100,0)/n,
    deadCurve: rs.reduce((a,r)=>a+r.curve.dead,0)/n,
    top5: rs.reduce((a,r)=>a+r.curve.top5,0)/n,
  };
}

function kpiTable(list){
  const a = summaryOf(list, "v0"), b = summaryOf(list, "v2");  const el = document.getElementById("kpi");
  if(!a || !b){ el.innerHTML = '<tr><td>没有匹配的档位</td></tr>'; return; }

  const slopeCls = v => v>=0.85?"ok":(v>=0.6?"warn":"bad");
  const dirCls = (v, lim) => Math.abs(v)<=lim ? "ok" : (Math.abs(v)<=lim*3 ? "warn":"bad");
  const rows = [
    ["Δ亮度均值", a.dMean.toFixed(3), b.dMean.toFixed(3),
      dirCls(a.dMean,0.03), dirCls(b.dMean,0.03), "偏离原图多少（0＝不改基调）"],
    ["绝对漂移", a.absD.toFixed(3), b.absD.toFixed(3),
      a.absD<=0.03?"ok":(a.absD<=0.1?"warn":"bad"),
      b.absD<=0.03?"ok":(b.absD<=0.1?"warn":"bad"), "逐图 |Δ| 的平均，越小越自然"],
    ["意图斜率", isNaN(a.slope)?"—":a.slope.toFixed(2),
      isNaN(b.slope)?"—":b.slope.toFixed(2), slopeCls(a.slope), slopeCls(b.slope),
      "1＝保留明暗意图；0＝全部拉成同一亮度"],
    ["层级保留", a.lvR.toFixed(2), b.lvR.toFixed(2),
      clsGoodHi(a.lvR,0.95,0.85), clsGoodHi(b.lvR,0.95,0.85), "1＝层次不变"],
    ["高频保留", a.hfR.toFixed(2), b.hfR.toFixed(2),
      clsGoodHi(a.hfR,0.95,0.85), clsGoodHi(b.hfR,0.95,0.85), "1＝细节不变"],
    ["截断Δ (pp)", (a.clipD>=0?"+":"")+a.clipD.toFixed(2),
      (b.clipD>=0?"+":"")+b.clipD.toFixed(2),
      a.clipD<=0?"ok":"bad", b.clipD<=0?"ok":"bad", "负＝救回高光，正＝又丢高光"],
    ["高光层级保留", (a.top5*100).toFixed(0)+"%", (b.top5*100).toFixed(0)+"%",
      clsGoodHi(a.top5,0.7,0.4), clsGoodHi(b.top5,0.7,0.4),
      "输入 250~255 这 5 级在输出里还剩几级（截断指标看不出差别时看这个）"],
    ["死黑Δ (pp)", (a.deadD>=0?"+":"")+a.deadD.toFixed(2),
      (b.deadD>=0?"+":"")+b.deadD.toFixed(2),
      a.deadD<=0.5?"ok":"bad", b.deadD<=0.5?"ok":"bad",
      "负＝救回暗部，正＝压死暗部（+0.5pp 以内肉眼不可辨）"],
    ["曲线死区", (a.deadCurve*100).toFixed(1)+"%", (b.deadCurve*100).toFixed(1)+"%",
      a.deadCurve<=0.05?"ok":(a.deadCurve<=0.3?"warn":"bad"),
      b.deadCurve<=0.05?"ok":(b.deadCurve<=0.3?"warn":"bad"),
      "灰阶曲线里斜率=0 的占比（层次被焊死的比例）"],
  ];
  el.innerHTML = `<thead><tr><th>指标</th><th class="old">现有方案</th>
    <th class="new">新方案</th><th>含义</th></tr></thead><tbody>` +
    rows.map(([k,x0,x2,c0,c2,h])=>
      `<tr><td>${k}</td><td class="${c0}">${x0}</td><td class="${c2}">${x2}</td>
       <td style="text-align:left;color:#6a7787;font-size:11.5px">${h}</td></tr>`).join("") +
    `</tbody>`;

  const s0 = summaryOf(list.filter(i=>i.variant!=="原片"), "v0");
  const s2 = summaryOf(list.filter(i=>i.variant!=="原片"), "v2");
  const n0 = document.getElementById("kpiNote");
  if(s0 && s2){
    n0.innerHTML = `<b>偏暗素材专项</b>（「想要的偏暗」能否保住）：` +
      `现有方案 Δ亮度 <b style="color:var(--bad)">${s0.dMean>=0?"+":""}${s0.dMean.toFixed(3)}</b>` +
      ` · 新方案 <b style="color:var(--ok)">${s2.dMean>=0?"+":""}${s2.dMean.toFixed(3)}</b>`;
  } else {
    // 原片视图下没有偏暗素材：不显示空占位，只留一句「切到偏暗模拟可看」。
    n0.innerHTML = `想看「偏暗意图能否保住」：把上面变体切到 <b>偏暗模拟</b>。`;
  }
}

function curveSvg(curve, color){
  const W=100,H=100,P=2;
  const path = curve.pts.map(([a,b],i)=>{
    const x = P + (a/255)*(W-2*P);
    const y = H - P - (b/255)*(H-2*P);
    return (i?"L":"M") + x.toFixed(1) + " " + y.toFixed(1);
  }).join(" ");
  return `<svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none">
    <line x1="0" y1="${H/2}" x2="${W}" y2="${H/2}" stroke="#2a333f" stroke-width="0.4" stroke-dasharray="2 2"/>
    <line x1="${W/2}" y1="0" x2="${W/2}" y2="${H}" stroke="#2a333f" stroke-width="0.4" stroke-dasharray="2 2"/>
    <path d="M${P} ${H-P} L${W-P} ${P}" fill="none" stroke="#3a4655" stroke-width="0.7"
          stroke-dasharray="3 3" vector-effect="non-scaling-stroke"/>
    <path d="${path}" fill="none" stroke="${color}" stroke-width="1.5" vector-effect="non-scaling-stroke"/>
  </svg>`;
}

function readout(it, eng, label){
  const r = run(it, eng);
  const d = r.m.mean - it.mIn.mean;
  const lvR = r.m.levels / Math.max(it.mIn.levels,1);
  const hfR = r.m.hf / Math.max(it.mIn.hf,1e-6);
  const clipD = (r.m.clipAny - it.mIn.clipAny)*100;
  const deadD = (r.m.dead - it.mIn.dead)*100;
  return `<div class="rd">
    <div class="hd">${label}</div>
    <div class="line">均值 <b>${r.m.mean.toFixed(3)}</b>
      <b class="${Math.abs(d)<=0.03?"ok":(Math.abs(d)<=0.1?"warn":"bad")}">${d>=0?"+":""}${d.toFixed(3)}</b></div>
    <div class="line">层级 <b class="${lvR>=0.95?"ok":(lvR>=0.85?"warn":"bad")}">${it.mIn.levels}→${r.m.levels} ×${lvR.toFixed(2)}</b></div>
    <div class="line">高频 <b class="${hfR>=0.95?"ok":(hfR>=0.85?"warn":"bad")}">×${hfR.toFixed(2)}</b>
      · 截断 <b class="${clipD<=0?"ok":(clipD<=1?"warn":"bad")}">${clipD>=0?"+":""}${clipD.toFixed(2)}pp</b></div>
    <div class="line">死黑 <b class="${deadD<=0.2?"ok":"bad"}">${deadD>=0?"+":""}${deadD.toFixed(2)}pp</b></div>
  </div>`;
}

function card(it){
  const r0 = run(it, "v0"), r2 = run(it, "v2");
  const d0 = r0.m.mean - it.mIn.mean, d2 = r2.m.mean - it.mIn.mean;
  return `<div class="card">
    <div class="head">
      <span class="name">${it.file}</span>
      <span class="tag ${it.variant!=="原片"?"dark":""}">${it.variant}</span>
      <span class="tag grade">${it.style} / ${it.strength}</span>
      <span class="tag">输入均值 ${it.mIn.mean.toFixed(3)}</span>
    </div>
    <div class="trio">
      <div class="col c-src">
        <img src="src/${it.skey}.jpg" alt="原图" loading="lazy">
        <div class="cap"><b>原图</b></div>
      </div>
      <div class="col c-old">
        <img src="v0/${it.ekey}.jpg" alt="现有方案" loading="lazy">
        <div class="cap"><b>现有方案</b>
          <span class="delta ${Math.abs(d0)<=0.03?"ok":"bad"}">${d0>=0?"+":""}${d0.toFixed(3)}</span></div>
      </div>
      <div class="col c-new">
        <img src="v2/${it.ekey}.jpg" alt="新方案" loading="lazy">
        <div class="cap"><b>新方案</b>
          <span class="delta ${Math.abs(d2)<=0.03?"ok":"bad"}">${d2>=0?"+":""}${d2.toFixed(3)}</span></div>
      </div>
    </div>
    <div class="readout">
      <div class="rd"><div class="hd">原图</div>
        <div class="line">均值 <b>${it.mIn.mean.toFixed(3)}</b></div>
        <div class="line">层级 <b>${it.mIn.levels}</b></div>
        <div class="line">高频 <b>${it.mIn.hf.toFixed(2)}</b> · 截断 <b>${(it.mIn.clipAny*100).toFixed(2)}%</b></div>
        <div class="line">死黑 <b>${(it.mIn.dead*100).toFixed(2)}%</b></div>
      </div>
      ${readout(it, "v0", "现有方案")}
      ${readout(it, "v2", "新方案")}
    </div>
    <div class="curves">
      <div class="cbox"><div class="ct">现有方案 · 灰阶响应</div>
        ${curveSvg(r0.curve, "var(--old)")}
        <div class="cf">死区 ${(r0.curve.dead*100).toFixed(1)}% ·
          动态范围 ${r0.curve.span.toFixed(0)}/255 ·
          高光 5 级保留 ${(r0.curve.top5*100).toFixed(0)}%</div></div>
      <div class="cbox"><div class="ct">新方案 · 灰阶响应</div>
        ${curveSvg(r2.curve, "var(--new)")}
        <div class="cf">死区 ${(r2.curve.dead*100).toFixed(1)}% ·
          动态范围 ${r2.curve.span.toFixed(0)}/255 ·
          高光 5 级保留 ${(r2.curve.top5*100).toFixed(0)}%</div></div>
    </div>
  </div>`;
}

function render(){
  const list = filtered();
  const shown = dedupe(list);
  const sorted = shown.slice().sort((a,b)=>
    a.file.localeCompare(b.file, "zh") ||
    (a.variant==="原片"?0:1) - (b.variant==="原片"?0:1) ||
    a.strength - b.strength || a.style.localeCompare(b.style));
  document.getElementById("grid").innerHTML = sorted.map(card).join("");

  const nFile = new Set(shown.map(i=>i.file)).size;
  const expanded = (state.style === "全部" && state.strength === "全部");
  document.getElementById("count").textContent = expanded
    ? `${shown.length} 组三列对比（${nFile} 张图 × 多档位）`
    : `${shown.length} 张图 · 每张 1 组三列对比`;

  kpiTable(list);
  updateHint(expanded);
}

/* 首屏提示：告诉用户「为什么只看到一组」，以及怎么展开其余档位。
   筛选为「原片」时偏暗专项无数据，两行提示合并显示，避免出现一条空占位。 */
function updateHint(expanded){
  let el = document.getElementById("expandHint");
  if(!el){
    el = document.createElement("div");
    el.id = "expandHint";
    el.style.cssText = "padding:0 26px 4px;color:#8b98a9;font-size:12px";
    document.querySelector(".summary").appendChild(el);
  }
  const total = ITEMS.length;
  el.innerHTML = expanded
    ? `已展开全部档位：共 <b>${total}</b> 组（每张图的每个风格/力度各一组）。`
    : `当前每张图只显示 1 组对比。全部档位共 <b>${total}</b> 组 —— 把「风格」和「力度」都选到「全部」即可展开矩阵。`;
}

document.getElementById("segVar").addEventListener("click", e=>{
  const b = e.target.closest("button"); if(!b) return;
  state.variant = b.dataset.var;
  document.querySelectorAll("#segVar button").forEach(x=>x.classList.toggle("on", x===b));
  render();
});
document.getElementById("selStyle").addEventListener("change", e=>{
  state.style = e.target.value; render();
});
document.getElementById("selStrength").addEventListener("change", e=>{
  state.strength = e.target.value; render();
});
render();
</script>
</body>
</html>
"""


def write_html(path: str, src: str, items: list[dict]) -> None:
    styles = sorted({it["style"] for it in items})
    strengths = sorted({it["strength"] for it in items})
    dark = next((it["variant"] for it in items if it["variant"] != "原片"), "偏暗模拟")
    # 默认档位取 natural（工坊默认配方用的就是它）；没有就退到第一个。
    def_style = "natural" if "natural" in styles else styles[0]
    def_strength = 0.85 if 0.85 in strengths else strengths[0]
    page = _PAGE
    page = page.replace("__SRCJSON__", json.dumps(src))
    page = page.replace("__SRC__", src)
    page = page.replace("__TIME__", time.strftime("%Y-%m-%d %H:%M"))
    page = page.replace("__NIMG__", str(len({it["file"] for it in items})))
    page = page.replace("__NITEM__", str(len(items)))
    page = page.replace("__DARKLABEL__", dark)
    page = page.replace("__DEFSTYLE__", json.dumps(def_style))
    page = page.replace("__DEFSTR__", json.dumps(float(def_strength)))
    page = page.replace("__STYLEOPTS__", '<option value="全部">全部（展开矩阵）</option>' + "".join(
        f'<option value="{s}"{" selected" if s == def_style else ""}>{s}</option>'
        for s in styles))
    page = page.replace("__STRENGTHOPTS__", '<option value="全部">全部（展开矩阵）</option>' + "".join(
        f'<option value="{s:g}"{" selected" if s == def_strength else ""}>{s:g}</option>'
        for s in strengths))
    page = page.replace("__DATA__", json.dumps(items, ensure_ascii=False,
                                                separators=(",", ":")))
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(page)


def main() -> int:
    ap = argparse.ArgumentParser(description="智能调色三列对比 HTML 报告")
    ap.add_argument("dir", help="照片目录")
    ap.add_argument("--styles", default="natural,vivid,warm,cool")
    ap.add_argument("--strengths", default="1.0,0.85")
    ap.add_argument("--lowkey", type=float, default=0.45,
                    help="额外生成一组「模拟偏暗」素材（RGB 乘该系数），0 = 不做")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--out", default=os.path.join(HERE, "out", "grade_tone_html"))
    args = ap.parse_args()

    src = os.path.abspath(args.dir)
    if not os.path.isdir(src):
        print(f"[错误] 目录不存在：{src}")
        return 2
    styles = [s.strip() for s in args.styles.split(",") if s.strip()]
    strengths = [float(s) for s in args.strengths.split(",") if s.strip()]
    bad = [s for s in styles if s not in v0.STYLES]
    if bad:
        print(f"[错误] 未知风格：{bad}，可选 {list(v0.STYLES)}")
        return 2

    files = sorted(p for p in os.listdir(src)
                   if os.path.isfile(os.path.join(src, p))
                   and os.path.splitext(p)[1].lower() in _IMG_EXT)
    if args.limit > 0:
        files = files[: args.limit]
    if not files:
        print(f"[错误] 目录内没有图片：{src}")
        return 2

    os.makedirs(args.out, exist_ok=True)
    print(f"素材 {len(files)} 张 · 风格 {styles} · 力度 {strengths} · 输出 {args.out}")
    t0 = time.time()
    items = collect(src, files, styles, strengths, args.lowkey, args.out)
    if not items:
        print("[错误] 没有可用素材")
        return 2

    write_html(os.path.join(args.out, "index.html"), src, items)
    print(f"\n完成：{len(items)} 组三列对比 / {time.time() - t0:.1f}s")
    for eng, label, _ in ENGINES:
        s = summarize(items, eng)
        print(f"  {label:<6} dMean={s['dMean']:+.3f}  斜率={s['slope']:>5}  "
              f"层级比={s['lvR']:.3f}  高频比={s['hfR']:.3f}  死区={s['deadD']:+.2f}pp")
    print(f"页面：{os.path.join(args.out, 'index.html')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
