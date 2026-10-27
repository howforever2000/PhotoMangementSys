"""把实库安全「快照」成单文件（无 WAL），供 Rust 量测探针读取。

用法：python .pi/snapshot_db.py
输出：%TEMP%\\pms_ro\\photos_clean.db
"""
import os
import sqlite3

src = os.path.join(os.environ["APPDATA"], "com.photomanagementsys.app", "photos.db")
out_dir = os.path.join(os.environ["TEMP"], "pms_ro")
os.makedirs(out_dir, exist_ok=True)
dst = os.path.join(out_dir, "photos_clean.db")
if os.path.exists(dst):
    os.remove(dst)

s = sqlite3.connect(f"file:{src}?mode=ro", uri=True)
d = sqlite3.connect(dst)
with d:
    s.backup(d)          # 一致性快照（含 WAL 中已提交的部分）
d.execute("PRAGMA journal_mode=DELETE")  # 输出成单文件，不要 WAL 旁文件
d.commit()
d.close()
s.close()
print(f"snapshot -> {dst}  {os.path.getsize(dst)/1024/1024:.1f} MB")
