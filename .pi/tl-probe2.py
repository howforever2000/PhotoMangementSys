# 时间线负载量测 v2：完全对齐 CONTENT_HIT_SELECT 的 18 列，量真实 JSON 体积
import json
import os
import sqlite3

db = os.path.join(os.environ["TEMP"], "pms_ro", "photos.db")
con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
con.row_factory = sqlite3.Row

SQL = """SELECT p.id, p.path, p.parent_dir, p.album_id, a.name, a.path,
            p.content, p.category, p.sub_category, p.label, p.confidence,
            p.person_ids, p.shoot_time, p.location, p.iso, p.aperture,
            p.shutter_speed, p.focal_length
     FROM photo_content_scan p
     LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
     WHERE p.user_id = 1
     ORDER BY (p.shoot_time IS NULL), p.shoot_time ASC, p.id ASC"""

keys = ["id", "path", "parent_dir", "album_id", "album_name", "album_path",
        "content", "category", "sub_category", "label", "confidence",
        "person_ids", "shoot_time", "location", "iso", "aperture",
        "shutter_speed", "focal_length"]

rows = [dict(zip(keys, tuple(r))) for r in con.execute(SQL)]
print(f"行数: {len(rows)}")

full = json.dumps(rows, ensure_ascii=False)
print(f"现 payload（18 列）: {len(full.encode('utf-8'))/1024/1024:.2f} MB")

# 时间线实际用到的 7 个字段
used_keys = ["id", "path", "album_id", "album_name", "shoot_time", "location", "label"]
lean = json.dumps([{k: r[k] for k in used_keys} for r in rows], ensure_ascii=False)
print(f"只留用到的 7 列: {len(lean.encode('utf-8'))/1024/1024:.2f} MB")

# 短键 + 数组化的极端情况（评估"可压到多小"）
cols_lean = [[r[k] for k in used_keys] for r in rows]
arr = json.dumps(cols_lean, ensure_ascii=False)
print(f"数组化 + 7 列: {len(arr.encode('utf-8'))/1024/1024:.2f} MB")

print("\n--- 各字段平均字符数 ---")
for k in keys:
    tot = sum(len(str(r[k])) for r in rows)
    print(f"  {k:14s} {tot/len(rows):7.1f}")

# 按月切片的体积（首屏只拿 5 个月时大概多大）
from collections import OrderedDict
by_month = OrderedDict()
for r in rows:
    ym = (r["shoot_time"] or "")[:7] or "未记录"
    by_month.setdefault(ym, []).append({k: r[k] for k in used_keys})
sizes = sorted(((len(json.dumps(v, ensure_ascii=False).encode("utf-8")) / 1024, k, len(v))
                for k, v in by_month.items()), reverse=True)
print(f"\n--- 按月切片（{len(sizes)} 个切片，7 列）---")
print(f"  最大月: {sizes[0][2]} 张 / {sizes[0][0]:.0f} KB  ({sizes[0][1]})")
print(f"  中位月: {sizes[len(sizes)//2][2]} 张 / {sizes[len(sizes)//2][0]:.0f} KB")
print(f"  前 5 个月合计: {sum(s[0] for s in sizes[:5]):.0f} KB")
con.close()
