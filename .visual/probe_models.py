import os, sqlite3, json, base64, requests

from cryptography.hazmat.primitives.ciphers.aead import AESGCM

appdata = os.path.join(os.environ["APPDATA"], "com.photomanagementsys.app")
key = open(os.path.join(appdata, "app.key"), "rb").read()
con = sqlite3.connect("file:" + os.path.join(appdata, "photos.db") + "?mode=ro", uri=True)
cfg = json.loads(con.execute("select value from app_settings where key='ai_studio:1'").fetchone()[0])
con.close()
enc = cfg["edit"]["api_key_enc"]
raw = base64.b64decode(enc[3:])
KEY = AESGCM(key).decrypt(raw[:12], raw[12:], None).decode()

BASE = "https://maas.qianwenaiapi.com/compatible-mode/v1"
H = {"Authorization": "Bearer " + KEY}

r = requests.get(BASE + "/models", headers=H, timeout=30)
print("GET /models ->", r.status_code)
try:
    j = r.json()
    data = j.get("data") or []
    print("model count:", len(data))
    ids = [m.get("id") for m in data if isinstance(m, dict)]
    for mid in ids:
        print("  -", mid)
    # highlight image-related
    print("IMAGE-RELATED:", [m for m in ids if m and ("image" in m.lower() or "img" in m.lower() or "wanx" in m.lower())])
except Exception as e:
    print("parse err", e, r.text[:300])
