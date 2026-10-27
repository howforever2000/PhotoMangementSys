# 校验：photo_content_scan.photo_hash 与 photo_thumb_cache.photo_hash 是否同源可 JOIN
import os
import sqlite3

db = os.path.join(os.environ["TEMP"], "pms_ro", "photos.db")
c = sqlite3.connect(f"file:{db}?mode=ro", uri=True)


def q(sql):
    return c.execute(sql).fetchone()[0]


print("scan rows user1     :", q("select count(*) from photo_content_scan where user_id=1"))
print("scan hash not empty :", q(
    "select count(*) from photo_content_scan where user_id=1 and photo_hash is not null and photo_hash<>''"))
print("thumb cache rows    :", q("select count(*) from photo_thumb_cache"))
print("JOIN matched rows   :", q(
    "select count(*) from photo_content_scan p join photo_thumb_cache t"
    " on t.photo_hash=p.photo_hash where p.user_id=1"))
print("JOIN distinct path  :", q(
    "select count(distinct p.path) from photo_content_scan p join photo_thumb_cache t"
    " on t.photo_hash=p.photo_hash where p.user_id=1"))
print("thumb_path empty    :", q(
    "select count(*) from photo_thumb_cache where thumb_path is null or thumb_path=''"))

# 未记录拍摄时间的分组大小
print("\n未记录组大小        :", q(
    "select count(*) from photo_content_scan where user_id=1 and (shoot_time is null or shoot_time='')"))
rows = c.execute(
    "select substr(shoot_time,1,7) ym, count(*) n from photo_content_scan"
    " where user_id=1 and shoot_time is not null and shoot_time<>''"
    " group by ym order by n desc limit 6"
).fetchall()
print("最大的 6 个月       :", rows)
c.close()
