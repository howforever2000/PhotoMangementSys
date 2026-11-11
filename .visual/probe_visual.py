import os, sqlite3, json, base64, io, requests
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from PIL import Image, ImageDraw

appdata = os.path.join(os.environ["APPDATA"], "com.photomanagementsys.app")
key = open(os.path.join(appdata, "app.key"), "rb").read()
con = sqlite3.connect("file:" + os.path.join(appdata, "photos.db") + "?mode=ro", uri=True)
cfg = json.loads(con.execute("select value from app_settings where key='ai_studio:1'").fetchone()[0])
con.close()
raw = base64.b64decode(cfg["edit"]["api_key_enc"][3:])
KEY = AESGCM(key).decrypt(raw[:12], raw[12:], None).decode()
BASE = "https://maas.qianwenaiapi.com/compatible-mode/v1"
H = {"Authorization": "Bearer " + KEY, "Content-Type": "application/json"}
OUT = os.path.dirname(os.path.abspath(__file__))

# Build a distinctive input: checkerboard + red diagonal + big text
im = Image.new("RGB", (512, 512), (255, 255, 255))
d = ImageDraw.Draw(im)
for y in range(0, 512, 64):
    for x in range(0, 512, 64):
        if (x // 64 + y // 64) % 2 == 0:
            d.rectangle([x, y, x + 63, y + 63], fill=(30, 60, 160))
d.line([(0, 0), (512, 512)], fill=(220, 30, 30), width=10)
d.rectangle([150, 200, 362, 312], outline=(0, 160, 60), width=12)
d.text((170, 240), "EDIT", fill=(0, 0, 0))
im.save(os.path.join(OUT, "input_pattern.png"))
buf = io.BytesIO(); im.save(buf, "PNG")
DATA_URL = "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()

def run(tag, body):
    r = requests.post(BASE + "/images/generations", headers=H, json=body, timeout=180)
    print(f"[{tag}] HTTP {r.status_code}")
    if r.status_code == 200:
        url = r.json()["data"][0]["url"]
        rd = requests.get(url, timeout=90)
        p = os.path.join(OUT, tag + ".png")
        open(p, "wb").write(rd.content)
        im2 = Image.open(io.BytesIO(rd.content))
        print("  saved", p, im2.size)
    else:
        print("  ", (r.text or "")[:200])

prompt = "make this a soft watercolor painting, keep the same composition and colors"
run("out_edit_with_input", {"model": "qwen-image-3.0-pro", "prompt": prompt, "n": 1,
                            "size": "512x512", "image": DATA_URL})
run("out_text_only", {"model": "qwen-image-3.0-pro", "prompt": prompt, "n": 1, "size": "512x512"})
