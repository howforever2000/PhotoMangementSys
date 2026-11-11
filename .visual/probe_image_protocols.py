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
H = {"Authorization": "Bearer " + KEY}

buf = io.BytesIO(); Image.new("RGB", (64, 64), (180, 90, 60)).save(buf, "PNG")
DATA_URL = "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()

def show(tag, r):
    print(f"\n=== {tag} -> HTTP {r.status_code} ct={r.headers.get('content-type','')}")
    txt = r.text or ""
    # don't dump huge base64
    if len(txt) > 600:
        print("body_len=", len(txt), "head=", txt[:400].replace("\n", " "))
    else:
        print("body=", txt)

def post(tag, body):
    try:
        r = requests.post(BASE + "/images/generations", headers={**H, "Content-Type": "application/json"}, json=body, timeout=120)
        show(tag, r)
    except Exception as e:
        print(f"\n=== {tag} -> EXC {type(e).__name__}: {e}")

print("STEP A: text2img")
post("A generations text2img", {"model": "qwen-image-3.0", "prompt": "a red apple on a table", "n": 1, "size": "1024*1024"})

print("\nSTEP B: img2img via generations+image")
post("B generations + image field", {"model": "qwen-image-3.0", "prompt": "turn this into a watercolor painting", "n": 1, "image": DATA_URL})

print("\nSTEP C: chat/completions multimodal")
body = {"model": "qwen-image-3.0",
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": "把这张图变成水彩画"},
            {"type": "image_url", "image_url": {"url": DATA_URL}},
        ]}],
        "modalities": ["image", "text"]}
try:
    r = requests.post(BASE + "/chat/completions", headers={**H, "Content-Type": "application/json"}, json=body, timeout=120)
    show("C chat/completions + modalities", r)
except Exception as e:
    print(f"\n=== C -> EXC {type(e).__name__}: {e}")
