"""诊断：缩略图缓存表与磁盘是否一致（BUG-2026-1028-014 的现场取证工具）。

背景：`photo_thumb_cache` 只是**缓存表**，行在 ≠ 文件在。时间线以前把
`thumb_path` 当事实下发，死链就渲染成空白卡片（前端兜底只看 thumb_path == null）。

用法：python python/bench/diag_thumb_cache_health.py
判读：
  - 「路径已不存在」占比高 → 时间线/网格会大量空白（本机实测 53.3%）；
  - 修复后该占比会随浏览下降（前端兜底会重生成并 upsert 覆盖死行）。
只读打开，不写库。
"""
import json
import os
import sqlite3
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

DB = os.path.join(
    os.environ["APPDATA"], "com.photomanagementsys.app", "photos.db"
)
print("DB:", DB, os.path.getsize(DB) // 1024 // 1024, "MB")

con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
cur = con.cursor()

cur.execute("SELECT COUNT(*) FROM photo_thumb_cache")
total = cur.fetchone()[0]
print("photo_thumb_cache 行数:", total)

cur.execute("SELECT thumb_path FROM photo_thumb_cache")
paths = [r[0] for r in cur.fetchall()]
missing = [p for p in paths if not p or not os.path.isfile(p)]
print(f"其中路径已不存在: {len(missing)} / {total} ({len(missing)*100.0/max(total,1):.1f}%)")
if missing:
    print("样例:", json.dumps(missing[:3], ensure_ascii=False))
    roots = {}
    for p in missing:
        roots[os.path.dirname(p or "")] = roots.get(os.path.dirname(p or ""), 0) + 1
    print("按目录统计:", json.dumps(roots, ensure_ascii=False, indent=2))

# 时间线「有 thumb_path」的行 → 有多少张的图片其实是坏的
cur.execute(
    """SELECT COUNT(*), SUM(CASE WHEN t.thumb_path IS NULL THEN 1 ELSE 0 END)
       FROM photo_content_scan p
       LEFT JOIN photo_thumb_cache t ON t.photo_hash = p.photo_hash"""
)
row = cur.fetchone()
print(f"photo_content_scan 总行数={row[0]} 其中 thumb_path 为 NULL={row[1]}")

# 抽 2000 条带 thumb_path 的，看磁盘命中率
cur.execute(
    """SELECT t.thumb_path
       FROM photo_content_scan p
       JOIN photo_thumb_cache t ON t.photo_hash = p.photo_hash
       LIMIT 2000"""
)
sample = [r[0] for r in cur.fetchall()]
hit = [p for p in sample if p and os.path.isfile(p)]
print(f"抽 {len(sample)} 条带 thumb_path：磁盘存在 {len(hit)}（{len(hit)*100.0/max(len(sample),1):.1f}%）")

# 缩略图目录本身
thedir = os.path.join(os.environ["APPDATA"], "com.photomanagementsys.app", "thumbs")
if os.path.isdir(thedir):
    n = sum(len(files) for _, _, files in os.walk(thedir))
    print("thumbs 目录文件数:", n, "→", thedir)
