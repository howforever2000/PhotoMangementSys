import os, sqlite3, json
a = os.path.join(os.environ["APPDATA"], "com.photomanagementsys.app")
c = sqlite3.connect("file:" + os.path.join(a, "photos.db") + "?mode=ro", uri=True)
cfg = json.loads(c.execute("select value from app_settings where key='ai_studio:1'").fetchone()[0])
c.close()
view = {k: ({kk: (vv if "key" not in kk.lower() else "<secret>") for kk, vv in v.items()} if isinstance(v, dict) else v) for k, v in cfg.items()}
print(json.dumps(view, ensure_ascii=False, indent=2))
