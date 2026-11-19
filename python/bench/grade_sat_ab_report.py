"""饱和度处理对照图：现有方案（RGB 线性） vs 只换 LCh（恒色相只动 C）。

产出 `sat_ab.html`（单文件，无外部依赖）：每张素材两列并排 + 差异读数。

用法：
    python python/bench/grade_sat_ab_report.py "<照片目录>"
        [--styles natural,vivid] [--limited 0] [--out python/bench/out/sat_ab]
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

import grade_lch_only as lch  # noqa: E402
from ai_studio import autograde as v0  # noqa: E402
from ai_studio import colortransfer as ct  # noqa: E402
from grade_tone_eval import dim, load_rgb, metrics  # noqa: E402

_PREVIEW_LONG = 760
_IMG_EXT = (".jpg", ".jpeg", ".png", ".bmp", ".webp", ".tif", ".tiff")


def fit(img: np.ndarray, long_edge: int = _PREVIEW_LONG) -> Image.Image:
    im = Image.fromarray(img)
    w, h = im.size
    m = max(w, h)
    if m > long_edge:
        s = long_edge / float(m)
        im = im.resize((max(1, round(w * s)), max(1, round(h * s))), Image.LANCZOS)
    return im


def color_metrics(src: np.ndarray, out: np.ndarray) -> dict:
    """只统计「颜色有意义」的像素（CIELAB 彩度 C>10）上的色相/明度/彩度变化。

    为什么用 CIELAB 的 atan2(b,a) 而不是 HSV 的色相角：HSV 在低饱和处噪声极大，
    实测同一组改动用 HSV 量出的色相偏移完全失真（0.47°→0.70°，方向反了）。
    """
    lab0 = ct.rgb_to_lab(src)
    lab1 = ct.rgb_to_lab(out)
    C0 = np.hypot(lab0[..., 1], lab0[..., 2])
    C1 = np.hypot(lab1[..., 1], lab1[..., 2])
    h0 = np.degrees(np.arctan2(lab0[..., 2], lab0[..., 1]))
    h1 = np.degrees(np.arctan2(lab1[..., 2], lab1[..., 1]))
    m = C0 > 10.0
    dh = ((h1[m] - h0[m] + 180.0) % 360.0) - 180.0
    return {
        "dL": round(float(np.abs(lab1[..., 0] - lab0[..., 0]).mean()), 4),
        "dLsign": round(float((lab1[..., 0] - lab0[..., 0]).mean()), 4),
        "dHue": round(float(np.abs(dh).mean()), 3),
        "dC": round(float((C1[m] - C0[m]).mean()), 3),
    }


def run(src: str, files: list[str], styles: list[str], out_dir: str) -> list[dict]:
    dirs = {k: os.path.join(out_dir, k) for k in ("src", "rgb", "lch")}
    for d in dirs.values():
        os.makedirs(d, exist_ok=True)

    items = []
    for i, name in enumerate(files, 1):
        try:
            before = load_rgb(os.path.join(src, name), long_edge=_PREVIEW_LONG)
        except Exception as e:  # noqa: BLE001
            print(f"  [{i}/{len(files)}] 跳过 {name}: {e}")
            continue
        m_in, _ = metrics(before)
        skey = f"{i:02d}"
        fit(before).save(os.path.join(dirs["src"], f"{skey}.jpg"), "JPEG", quality=90)

        for style in styles:
            runs = []
            for eng, fn in (("rgb", lambda a, s: v0.grade(a, style=s, strength=1.0)[0]),
                            ("lch", lambda a, s: lch.grade_lch(a, style=s, strength=1.0)[0])):
                out = fn(before, style)
                m_out, _ = metrics(out)
                ekey = f"{skey}_{style}"
                fit(out).save(os.path.join(dirs[eng], f"{ekey}.jpg"), "JPEG", quality=90)
                runs.append({"eng": eng, "m": m_out,
                             "color": color_metrics(before, out)})
            items.append({"file": name, "style": style, "skey": skey,
                          "ekey": f"{skey}_{style}", "mIn": m_in, "runs": runs})
        print(f"  [{i}/{len(files)}] {name}")
    return items


def summary(items: list[dict], eng: str) -> dict:
    if not items:
        return {}
    sel = [next(r for r in it["runs"] if r["eng"] == eng) for it in items]
    return {
        "dL": round(float(np.mean([r["color"]["dL"] for r in sel])), 4),
        "dLsign": round(float(np.mean([r["color"]["dLsign"] for r in sel])), 4),
        "dHue": round(float(np.mean([r["color"]["dHue"] for r in sel])), 3),
        "dC": round(float(np.mean([r["color"]["dC"] for r in sel])), 3),
        "lvR": round(float(np.mean([r["m"]["levels"] / max(it["mIn"]["levels"], 1)
                                    for it, r in zip(items, sel)])), 3),
    }


_PAGE = r"""<!DOCTYPE html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>饱和度处理对照 · RGB 线性 vs LCh 恒色相</title>
<style>
:root{--bg:#0d1014;--panel:#151a21;--panel2:#1b222b;--line:#2a333f;--fg:#e8edf4;
 --dim:#8b98a9;--accent:#5aa9ff;--ok:#3ecf8e;--warn:#f5b942;--bad:#ff6b6b;
 --old:#c98b4b;--new:#5aa9ff}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);
 font:14px/1.6 "Segoe UI","Microsoft YaHei",system-ui,sans-serif}
header{padding:22px 26px 14px;border-bottom:1px solid var(--line);background:var(--panel)}
h1{margin:0 0 6px;font-size:19px;font-weight:600}
.sub{color:var(--dim);font-size:12.5px}
.sub code{background:#000;padding:1px 5px;border-radius:4px;color:#9fd0ff}
.bar{position:sticky;top:0;z-index:20;display:flex;flex-wrap:wrap;gap:10px 16px;
 align-items:center;padding:12px 26px;background:rgba(21,26,33,.96);
 border-bottom:1px solid var(--line);backdrop-filter:blur(8px)}
.bar label{color:var(--dim);font-size:12px;margin-right:4px}
.bar select{background:var(--panel2);color:var(--fg);border:1px solid var(--line);
 border-radius:6px;padding:5px 8px;font-size:13px}
.count{color:var(--dim);font-size:12px;margin-left:auto}
.summary{padding:18px 26px;border-bottom:1px solid var(--line)}
table.kpi{border-collapse:collapse;width:100%;max-width:820px;font-size:13px}
table.kpi th,table.kpi td{padding:7px 12px;border-bottom:1px solid var(--line);
 text-align:right;font-variant-numeric:tabular-nums}
table.kpi th:first-child,table.kpi td:first-child{text-align:left;color:var(--dim);
 font-weight:400;font-size:12.5px}
table.kpi thead th{color:var(--dim);font-weight:500;font-size:12px}
table.kpi thead th.old{color:var(--old)} table.kpi thead th.new{color:var(--new)}
td.ok{color:var(--ok)} td.warn{color:var(--warn)} td.bad{color:var(--bad)}
.grid{display:flex;flex-direction:column;gap:20px;padding:20px 26px 60px}
.card{background:var(--panel);border:1px solid var(--line);border-radius:12px;overflow:hidden}
.head{display:flex;align-items:baseline;gap:8px;padding:11px 14px 9px;flex-wrap:wrap}
.name{font-size:13.5px;font-weight:600}
.tag{font-size:11px;padding:1px 7px;border-radius:20px;border:1px solid var(--line);
 color:var(--dim);white-space:nowrap}
.tag.grade{color:var(--accent);border-color:#22405e}
.duo{display:grid;grid-template-columns:1fr 1fr;gap:1px;background:var(--line)}
.col{background:#000;position:relative;line-height:0}
.col img{display:block;width:100%;height:auto}
.col .cap{position:absolute;top:0;left:0;right:0;padding:6px 9px;font-size:11.5px;
 background:linear-gradient(180deg,rgba(0,0,0,.78),rgba(0,0,0,0))}
.col.c-old .cap b{color:var(--old)} .col.c-new .cap b{color:var(--new)}
.ro{display:grid;grid-template-columns:1fr 1fr;gap:1px;background:var(--line);
 border-top:1px solid var(--line)}
.rd{background:var(--panel);padding:9px 12px;font-size:12px}
.rd .hd{font-size:11px;color:var(--dim);margin-bottom:3px}
.rd .line{color:var(--dim);font-variant-numeric:tabular-nums}
.rd .line b{color:var(--fg)} .rd .line b.ok{color:var(--ok)}
.rd .line b.warn{color:var(--warn)} .rd .line b.bad{color:var(--bad)}
.note{padding:0 26px 30px;color:var(--dim);font-size:12px}
.note h3{color:var(--fg);font-size:13.5px;margin:22px 0 6px}
.note ul{margin:6px 0;padding-left:20px} .note li{margin:4px 0}
.note code{background:#000;padding:1px 5px;border-radius:4px;color:#9fd0ff;font-size:11.5px}
.zoom{display:grid;grid-template-columns:1fr 1fr;gap:1px;background:var(--line);
 border-top:1px solid var(--line)}
.zoom .z{background:#000;position:relative;overflow:hidden}
.zoom img{display:block;width:100%;height:auto}
.hint{padding:6px 14px 10px;color:var(--dim);font-size:11.5px}
</style></head>
<body>
<header>
  <h1>饱和度处理对照 · RGB 线性 vs LCh 恒色相</h1>
  <div class="sub">
    素材 <code>__SRC__</code> · __TIME__ · 共 __NIMG__ 张<br>
    <b>只改饱和度这一步</b>：左＝现有方案（RGB 里 <code>luma+(x-luma)*k</code>），
    右＝LCh（固定色相角与明度，只缩放彩度 C）。影调、白平衡、高光收口全部原样保留。
  </div>
</header>
<div class="bar">
  <div><label>风格</label><select id="selStyle">__STYLEOPTS__</select></div>
  <span class="count" id="count"></span>
</div>
<div class="summary"><table class="kpi" id="kpi"></table></div>
<div class="grid" id="grid"></div>
<div class="note">
  <h3>这次改动到底改了什么</h3>
  <ul>
    <li>只动 <code>eval_colors</code> 的第 5、6 步（饱和度 / 自然饱和度）。其余六步
      与现有方案逐行一致，所以两组出片的差异可 100% 归因于色彩空间。</li>
    <li><b>RGB 线性缩放</b>：<code>l + (x-l)*k</code>。通道被推出 [0,1] 后硬裁，
      裁掉的部分同时改变明度与色相 —— 这就是「拉饱和画面整体变亮」的来源。</li>
    <li><b>LCh</b>：在 CIELAB 里固定色相角 h 与明度 L，只把彩度 C 乘 k。
      超色域时用<b>二分求出的 headroom</b> 回收倍数，宁可少提也不让色相被裁歪。</li>
    <li><b>架构不变</b>：LCh 是逐色纯函数，只在建 33³ LUT 时对 35937 个格点求值
      （约 36ms），之后仍走原查表路径 —— <code>_lut_digest</code> 缓存、
      <code>.cube</code> 导出、97³ 稠密表全部照旧。</li>
  </ul>
  <h3>怎么读读数</h3>
  <ul>
    <li><b>|ΔL|</b>：CIELAB 明度漂移（拉饱和顺带变亮多少）—— 越小越干净。</li>
    <li><b>|ΔHue|</b>：CIELAB 色相角偏移（度）。只统计彩度 C&gt;10 的像素，
      避开低饱和噪声（HSV 色相在那种地方完全不可信）。</li>
    <li><b>ΔC</b>：彩度变化量。两组都为正说明都在提饱和，量级接近才可比。</li>
  </ul>
</div>
<script>
const ITEMS = __DATA__;
const state = { style: "__DEFSTYLE__" };
function run(it,e){ return it.runs.find(r=>r.eng===e); }
function cls(v,good,bad){ return v<=good?"ok":(v<=bad?"warn":"bad"); }
function filtered(){ return ITEMS.filter(it=>it.style===state.style); }
function summaryOf(list,eng){
  if(!list.length) return null;
  const rs=list.map(i=>run(i,eng));
  const f=(k)=>rs.reduce((a,r)=>a+r.color[k],0)/rs.length;
  return {dL:f("dL"), dLsign:f("dLsign"), dHue:f("dHue"), dC:f("dC"),
    lvR: rs.reduce((a,r,i)=>a+r.m.levels/Math.max(list[i].mIn.levels,1),0)/rs.length};
}
function kpi(list){
  const a=summaryOf(list,"rgb"), b=summaryOf(list,"lch");
  if(!a||!b){ document.getElementById("kpi").innerHTML="<tr><td>无数据</td></tr>"; return; }
  const rows=[
    ["|ΔL| 明度漂移", a.dL.toFixed(4), b.dL.toFixed(4), cls(a.dL,0.12,0.20), cls(b.dL,0.12,0.20),
      "拉饱和顺带改变明度的幅度，越小越干净"],
    ["ΔL 带符号", (a.dLsign>=0?"+":"")+a.dLsign.toFixed(4),
      (b.dLsign>=0?"+":"")+b.dLsign.toFixed(4),
      Math.abs(a.dLsign)<=0.06?"ok":"bad", Math.abs(b.dLsign)<=0.06?"ok":"bad",
      "正＝整体变亮（现行方案的系统性偏差）"],
    ["|ΔHue| 色相偏移°", a.dHue.toFixed(3), b.dHue.toFixed(3),
      cls(a.dHue,0.3,0.6), cls(b.dHue,0.3,0.6), "CIELAB 色相角，只统计 C>10 的像素"],
    ["ΔC 彩度变化", (a.dC>=0?"+":"")+a.dC.toFixed(3), (b.dC>=0?"+":"")+b.dC.toFixed(3),
      "", "", "两组都应为正且量级接近，否则不可比"],
    ["层级保留", a.lvR.toFixed(3), b.lvR.toFixed(3),
      a.lvR>=0.95?"ok":"bad", b.lvR>=0.95?"ok":"bad", "调色后剩下的亮度级数比例"],
  ];
  document.getElementById("kpi").innerHTML =
    `<thead><tr><th>指标</th><th class="old">现有方案（RGB）</th>
     <th class="new">LCh 恒色相</th><th>含义</th></tr></thead><tbody>` +
    rows.map(([k,x,y,c1,c2,h])=>`<tr><td>${k}</td><td class="${c1}">${x}</td>
     <td class="${c2}">${y}</td>
     <td style="text-align:left;color:#6a7787;font-size:11.5px">${h}</td></tr>`).join("") +
    `</tbody>`;
}
function card(it){
  const a=run(it,"rgb"), b=run(it,"lch");
  return `<div class="card">
    <div class="head"><span class="name">${it.file}</span>
      <span class="tag grade">${it.style}</span>
      <span class="tag">输入亮度 ${it.mIn.mean.toFixed(3)}</span></div>
    <div class="duo">
      <div class="col c-old"><img src="rgb/${it.ekey}.jpg" alt="现有" loading="lazy">
        <div class="cap"><b>现有方案 · RGB 线性缩放</b></div></div>
      <div class="col c-new"><img src="lch/${it.ekey}.jpg" alt="LCh" loading="lazy">
        <div class="cap"><b>LCh 恒色相只动 C</b></div></div>
    </div>
    <div class="ro">
      <div class="rd"><div class="hd">现有方案（RGB）</div>
        <div class="line">|ΔL| <b class="${cls(a.color.dL,0.12,0.20)}">${a.color.dL.toFixed(4)}</b>
          · 带符号 <b class="${Math.abs(a.color.dLsign)<=0.06?"ok":"bad"}">${a.color.dLsign>=0?"+":""}${a.color.dLsign.toFixed(4)}</b></div>
        <div class="line">|ΔHue| <b class="${cls(a.color.dHue,0.3,0.6)}">${a.color.dHue.toFixed(3)}°</b>
          · ΔC <b>${a.color.dC>=0?"+":""}${a.color.dC.toFixed(2)}</b></div>
        <div class="line">亮度 <b>${it.mIn.mean.toFixed(3)}→${a.m.mean.toFixed(3)}</b></div></div>
      <div class="rd"><div class="hd">LCh 恒色相</div>
        <div class="line">|ΔL| <b class="${cls(b.color.dL,0.12,0.20)}">${b.color.dL.toFixed(4)}</b>
          · 带符号 <b class="${Math.abs(b.color.dLsign)<=0.06?"ok":"bad"}">${b.color.dLsign>=0?"+":""}${b.color.dLsign.toFixed(4)}</b></div>
        <div class="line">|ΔHue| <b class="${cls(b.color.dHue,0.3,0.6)}">${b.color.dHue.toFixed(3)}°</b>
          · ΔC <b>${b.color.dC>=0?"+":""}${b.color.dC.toFixed(2)}</b></div>
        <div class="line">亮度 <b>${it.mIn.mean.toFixed(3)}→${b.m.mean.toFixed(3)}</b></div></div>
    </div>
  </div>`;
}
function render(){
  const list=filtered();
  document.getElementById("grid").innerHTML=list.map(card).join("");
  document.getElementById("count").textContent=`${list.length} 组两列对比`;
  kpi(list);
}
document.getElementById("selStyle").addEventListener("change",e=>{
  state.style=e.target.value; render();
});
render();
</script></body></html>
"""


def write_html(path: str, src: str, items: list[dict]) -> None:
    styles = sorted({it["style"] for it in items})
    def_style = "natural" if "natural" in styles else styles[0]
    page = _PAGE
    page = (page.replace("__SRC__", src)
                .replace("__TIME__", time.strftime("%Y-%m-%d %H:%M"))
                .replace("__NIMG__", str(len({it["file"] for it in items})))
                .replace("__DEFSTYLE__", def_style)
                .replace("__STYLEOPTS__", "".join(
                    f'<option value="{s}"{" selected" if s == def_style else ""}>{s}</option>'
                    for s in styles))
                .replace("__DATA__", json.dumps(items, ensure_ascii=False,
                                                separators=(",", ":"))))
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(page)


def main() -> int:
    ap = argparse.ArgumentParser(description="饱和度对照报告")
    ap.add_argument("dir")
    ap.add_argument("--styles", default="natural,vivid")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--out", default=os.path.join(HERE, "out", "sat_ab"))
    args = ap.parse_args()

    src = os.path.abspath(args.dir)
    files = sorted(p for p in os.listdir(src)
                   if os.path.isfile(os.path.join(src, p))
                   and os.path.splitext(p)[1].lower() in _IMG_EXT)
    if args.limit > 0:
        files = files[: args.limit]
    if not files:
        print(f"[错误] 目录内没有图片：{src}")
        return 2
    styles = [s.strip() for s in args.styles.split(",") if s.strip()]

    os.makedirs(args.out, exist_ok=True)
    print(f"素材 {len(files)} 张 · 风格 {styles} · 输出 {args.out}")
    t0 = time.time()
    items = run(src, files, styles, args.out)
    write_html(os.path.join(args.out, "index.html"), src, items)
    print(f"\n完成 {len(items)} 组 / {time.time() - t0:.1f}s")
    for eng, label in (("rgb", "现有 RGB"), ("lch", "LCh 恒色相")):
        s = summary(items, eng)
        print(f"  {label:<12} |ΔL|={s['dL']:.4f}  带符号={s['dLsign']:+.4f}  "
              f"|ΔHue|={s['dHue']:.3f}°  ΔC={s['dC']:+.3f}")
    print(f"页面：{os.path.join(args.out, 'index.html')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
