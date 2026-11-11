import os, sqlite3, json, base64, io, requests
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from PIL import Image

appdata = os.path.join(os.environ["APPDATA"], "com.photomanagementsys.app")
key = open(os.path.join(appdata, "app.key"), "rb").read()
con = sqlite3.connect("file:" + os.path.join(appdata, "photos.db") + "?mode=ro", uri=True)
cfg = json.loads(con.execute("select value from app_settings where key='ai_studio:1'").fetchone()[0])
con.close()
raw = base64.b64decode(cfg["edit"]["api_key_enc"][3:])
KEY = AESGCM(key).decrypt(raw[:12], raw[12:], None).decode()
BASE = "https://maas.qianwenaiapi.com/compatible-mode/v1"
H = {"Authorization": "Bearer " + KEY, "Content-Type": "application/json"}

def short(r):
    t = r.text or ""
    if len(t) > 220:
        t = t[:220].replace("\n", " ")
    return f"HTTP {r.status_code} {t}"

print("== text model qwen-flash ==")
try:
    r = requests.post(BASE + "/chat/completions", headers=H,
        json={"model": "qwen-flash", "messages": [{"role": "user", "content": "say ok"}], "max_tokens": 5}, timeout=60)
    print("qwen-flash:", short(r))
except Exception as e:
    print("qwen-flash EXC", e)

print("\n== image models text2img 512 ==")
for m in ["qwen-image-3.0", "qwen-image-3.0-pro", "qwen-image-2.1-pro", "qwen-image-2.0",
          "wan2.7-image", "wan2.7-image-pro", "z-image-turbo"]:
    try:
        r = requests.post(BASE + "/images/generations", headers=H,
            json={"model": m, "prompt": "a red apple", "n": 1, "size": "512*512"}, timeout=120)
        print(m, "->", short(r))
    except Exception as e:
        print(m, "EXC", type(e).__name__, e)
