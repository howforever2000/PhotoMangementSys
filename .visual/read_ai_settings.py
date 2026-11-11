import sqlite3, json

db = r"C:\Users\Admin\AppData\Roaming\com.photomanagementsys.app\photos.db"
con = sqlite3.connect("file:" + db + "?mode=ro", uri=True)
cur = con.cursor()
try:
    rows = cur.execute("select key,value from app_settings where key like 'ai_studio%'").fetchall()
except Exception as e:
    print("ERR reading app_settings:", e)
    rows = []

def mask(o):
    if isinstance(o, dict):
        out = {}
        for k, v in o.items():
            if "key" in k.lower() and isinstance(v, str) and v:
                out[k] = "<masked len=%d>" % len(v)
            else:
                out[k] = mask(v)
        return out
    if isinstance(o, list):
        return [mask(x) for x in o]
    return o

for k, v in rows:
    print("KEY=", k)
    try:
        d = json.loads(v)
        print(json.dumps(mask(d), ensure_ascii=False, indent=2))
    except Exception:
        print("raw:", v[:500])
con.close()
