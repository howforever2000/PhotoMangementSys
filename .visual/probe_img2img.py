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

buf = io.BytesIO(); Image.new("RGB", (96, 96), (180, 90, 60)).save(buf, "PNG")
DATA_URL = "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()

def show(tag, r):
    t = (r.text or "")
    print(f"\n[{tag}] HTTP {r.status_code} len={len(t)}")
    if r.status_code == 200:
        try:
            j = r.json(); d = (j.get("data") or [{}])[0]
            print("  data[0] keys:", list(d.keys()))
            url = d.get("url"); b64 = d.get("b64_json")
            if url:
                rd = requests.get(url, timeout=60)
                print("  downloaded:", rd.status_code, len(rd.content), "bytes")
                p = os.path.join(os.path.dirname(__file__), tag.replace(" ", "_").replace("/", "_") + ".png")
                open(p, "wb").write(rd.content)
                im = Image.open(io.BytesIO(rd.content)); print("  image:", im.size, im.mode)
            elif b64:
                print("  b64 len:", len(b64))
        except Exception as e:
            print("  parse err:", e, t[:200])
    else:
        print(" ", t[:260].replace("\n", " "))

def gen(field_name, model="qwen-image-3.0-pro", extra=None, tag=""):
    body = {"model": model, "prompt": "turn this into a watercolor painting", "n": 1, "size": "512x512"}
    if field_name:
        body[field_name] = [DATA_URL] if field_name == "images" else DATA_URL
    if extra: body.update(extra)
    try:
        r = requests.post(BASE + "/images/generations", headers=H, json=body, timeout=150)
        show(tag, r)
    except Exception as e:
        print(f"\n[{tag}] EXC {type(e).__name__}: {e}")

# 1) text2img 3.0-pro to get a real OSS url
print("== get a fresh OSS url ==")
r = requests.post(BASE + "/images/generations", headers=H,
                  json={"model": "qwen-image-3.0-pro", "prompt": "a red apple on white", "n": 1, "size": "512x512"}, timeout=150)
oss = None
if r.status_code == 200:
    oss = r.json()["data"][0].get("url")
    print("got oss url:", (oss or "")[:80], "...")
else:
    print("text gen failed", r.status_code, r.text[:200])

gen("image", tag="img2img_image_field")
gen("images", tag="img2img_images_array")

if oss:
    body = {"model": "qwen-image-2.0", "prompt": "make watercolor", "n": 1, "size": "512x512", "image": oss}
    rr = requests.post(BASE + "/images/generations", headers=H, json=body, timeout=150)
    show("qwen2.0_with_oss_url", rr)

# /images/edits with 3.0-pro, tiny body (expect 404 route)
try:
    files = [("image", ("a.png", buf.getvalue(), "image/png"))]
    r = requests.post(BASE + "/images/edits", headers={"Authorization": "Bearer " + KEY},
                      files=files, data={"model": "qwen-image-3.0-pro", "prompt": "x", "n": "1"}, timeout=60)
    print("\n[/images/edits 3.0-pro tiny] HTTP", r.status_code, repr((r.text or "")[:120]))
except Exception as e:
    print("\n[/images/edits 3.0-pro tiny] EXC", type(e).__name__, e)
