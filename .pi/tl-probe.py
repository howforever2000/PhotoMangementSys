# 时间线负载量测：list_timeline 的 JSON 体积与字段构成
# 只读副本，不改原库（副本在 %TEMP%\pms_ro\photos.db）
import json
import os
import sqlite3
import sys

db = os.path.join(os.environ["TEMP"], "pms_ro", "photos.db")
con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
con.row_factory = sqlite3.Row

print("--- photo_content_scan 行数 ---")
for r in con.execute(
    "SELECT user_id, COUNT(*) c FROM photo_content_scan GROUP BY user_id ORDER BY c DESC LIMIT 5"
):
    print(f"user {r['user_id']}: {r['c']} 行")

# 模拟 CONTENT_HIT_SELECT 的字段（尽量贴近 map_content_search_hit）
cols = [c[1] for c in con.execute("PRAGMA table_info(photo_content_scan)")]
print("\n--- photo_content_scan 列 ---")
print(", ".join(cols))

rows = con.execute(
    "SELECT id, path, album_id, shoot_time, location, category, sub_category, label, confidence "
    "FROM photo_content_scan WHERE user_id = 1"
).fetchall()
print(f"\n--- user=1 行数: {len(rows)} ---")

payload = [
    {
        "id": r["id"],
        "path": r["path"],
        "album_id": r["album_id"],
        "shoot_time": r["shoot_time"],
        "location": r["location"],
        "category": r["category"],
        "sub_category": r["sub_category"],
        "label": r["label"],
        "confidence": r["confidence"],
    }
    for r in rows
]
js = json.dumps(payload, ensure_ascii=False)
print(f"JSON 体积: {len(js)/1024/1024:.2f} MB（UTF-8：{len(js.encode('utf-8'))/1024/1024:.2f} MB）")

# 字段平均长度
import statistics

def avg(key):
    vals = [len(str(p[key] or "")) for p in payload]
    return sum(vals) / len(vals)

print("\n--- 字段平均字符长度 ---")
for k in ("path", "shoot_time", "location", "category", "sub_category", "label", "album_id"):
    print(f"  {k}: {avg(k):.1f}")

albums = {p["album_id"] for p in payload}
print(f"\n涉及相册数: {len(albums)}")
months = {(p["shoot_time"] or "")[:7] for p in payload}
print(f"涉及年月分组数: {len(months)}")

no_time = sum(1 for p in payload if not p["shoot_time"])
print(f"无拍摄时间: {no_time}")

# 缩略图缓存表
print("\n--- photo_thumb_cache ---")
try:
    n = con.execute("SELECT COUNT(*) FROM photo_thumb_cache").fetchone()[0]
    print(f"行数: {n}")
    for r in con.execute("SELECT thumb_path FROM photo_thumb_cache LIMIT 3"):
        print("  样例:", r[0])
    n_webp = con.execute(
        "SELECT COUNT(*) FROM photo_thumb_cache WHERE thumb_path LIKE '%.webp'"
    ).fetchone()[0]
    n_jpg = con.execute(
        "SELECT COUNT(*) FROM photo_thumb_cache WHERE thumb_path LIKE '%.jpg'"
    ).fetchone()[0]
    print(f"  .webp: {n_webp}   .jpg: {n_jpg}")
except sqlite3.Error as e:
    print("读表失败:", e)

con.close()
