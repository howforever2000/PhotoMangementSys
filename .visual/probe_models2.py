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

buf = io.BytesIO(); Image.new("RGB", (64, 64), (180, 90, 60)).save(buf, "PNG")
DATA_URL = "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()

def show(tag, r):
    t = (r.text or "")[:260].replace("\n", " ")
    print(f"{tag} -> HTTP {r.status_code} {t}")

cases = [
    ("3.0-pro 512x512 text", {"model": "qwen-image-3.0-pro", "prompt": "a red apple", "n": 1, "size": "512x512"}),
    ("2.1-pro 512x512 text", {"model": "qwen-image-2.1-pro", "prompt": "a red apple", "n": 1, "size": "512x512"}),
    ("2.0 512x512 text", {"model": "qwen-image-2.0", "prompt": "a red apple", "n": 1, "size": "512x512"}),
    ("2.0 512x512 + image(dataurl)", {"model": "qwen-image-2.0", "prompt": "make watercolor", "n": 1, "size": "512x512", "image": DATA_URL}),
    ("wan2.7-image 512x512 + image(dataurl)", {"model": "wan2.7-image", "prompt": "make watercolor", "n": 1, "size": "512x512", "image": DATA_URL}),
]
for tag, body in cases:
    try:
        r = requests.post(BASE + "/images/generations", headers=H, json=body, timeout=120)
        show(tag, r)
    except Exception as e:
        print(tag, "EXC", type(e).__name__, e)
