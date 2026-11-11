import os, sqlite3, json, base64, io, re, requests
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from PIL import Image, ImageDraw

OUT = os.path.dirname(os.path.abspath(__file__))
appdata = os.path.join(os.environ["APPDATA"], "com.photomanagementsys.app")
key = open(os.path.join(appdata, "app.key"), "rb").read()
con = sqlite3.connect("file:" + os.path.join(appdata, "photos.db") + "?mode=ro", uri=True)
cfg = json.loads(con.execute("select value from app_settings where key='ai_studio:1'").fetchone()[0])
con.close()
raw = base64.b64decode(cfg["edit"]["api_key_enc"][3:])
KEY = AESGCM(key).decrypt(raw[:12], raw[12:], None).decode()
GW = "https://maas.qianwenaiapi.com"
H = {"Authorization": "Bearer " + KEY, "Content-Type": "application/json"}

# 1K input pattern (1024x768)
im = Image.new("RGB", (1024, 768), (255, 255, 255))
d = ImageDraw.Draw(im)
for y in range(0, 768, 96):
    for x in range(0, 1024, 96):
        if (x // 96 + y // 96) % 2 == 0:
            d.rectangle([x, y, x + 95, y + 95], fill=(30, 60, 160))
d.line([(0, 0), (1024, 768)], fill=(220, 30, 30), width=14)
d.rectangle([300, 300, 720, 470], outline=(0, 160, 60), width=16)
im.save(os.path.join(OUT, "input_1k.png"))
buf = io.BytesIO(); im.save(buf, "JPEG", quality=88)
DATA_URL = "data:image/jpeg;base64," + base64.b64encode(buf.getvalue()).decode()
print("input jpeg bytes:", len(buf.getvalue()))

def extract_url(obj):
    urls = []
    def walk(o):
        if isinstance(o, dict):
            for k, v in o.items():
                if k in ("url", "image") and isinstance(v, str) and v.startswith("http"):
                    urls.append(v)
                else:
                    walk(v)
        elif isinstance(o, list):
            for x in o: walk(x)
    walk(obj)
    return urls

def run(tag, url, body):
    try:
        r = requests.post(url, headers=H, json=body, timeout=240)
        t = r.text or ""
        print(f"\n[{tag}] HTTP {r.status_code} len={len(t)}")
        if r.status_code == 200:
            try:
                j = r.json()
            except Exception:
                print("  non-json:", t[:200]); return
            urls = extract_url(j)
            print("  urls:", [u[:70] for u in urls])
            if urls:
                rd = requests.get(urls[0], timeout=90)
                p = os.path.join(OUT, tag + ".png")
                open(p, "wb").write(rd.content)
                im2 = Image.open(io.BytesIO(rd.content))
                print("  saved", os.path.basename(p), im2.size, len(rd.content), "bytes")
            else:
                print("  json head:", json.dumps(j, ensure_ascii=False)[:300])
        else:
            print("  ", t[:300].replace("\n", " "))
    except Exception as e:
        print(f"\n[{tag}] EXC {type(e).__name__}: {e}")

prompt = "把这张图变成柔和的水彩风格，保持构图和颜色不变"

# A) images/generations with x size
run("A_img_gen_x", GW + "/compatible-mode/v1/images/generations",
    {"model": "qwen-image-3.0", "prompt": prompt, "n": 1, "size": "1024x768", "image": DATA_URL})
# A2) * size
run("A2_img_gen_star", GW + "/compatible-mode/v1/images/generations",
    {"model": "qwen-image-3.0", "prompt": prompt, "n": 1, "size": "1024*768", "image": DATA_URL})
# B) chat/completions with modalities
run("B_chat_modalities", GW + "/compatible-mode/v1/chat/completions",
    {"model": "qwen-image-3.0",
     "messages": [{"role": "user", "content": [
         {"type": "text", "text": prompt},
         {"type": "image_url", "image_url": {"url": DATA_URL}}]}],
     "modalities": ["image", "text"]})
# B2) chat/completions without modalities
run("B2_chat_plain", GW + "/compatible-mode/v1/chat/completions",
    {"model": "qwen-image-3.0",
     "messages": [{"role": "user", "content": [
         {"type": "text", "text": prompt},
         {"type": "image_url", "image_url": {"url": DATA_URL}}]}]})
# C) native multimodal-generation
run("C_native", GW + "/api/v1/services/aigc/multimodal-generation/generation",
    {"model": "qwen-image-3.0",
     "input": {"messages": [{"role": "user", "content": [
         {"image": DATA_URL}, {"text": prompt}]}]},
     "parameters": {"n": 1}})
