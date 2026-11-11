import os, sys, json, sqlite3, base64
sys.path.insert(0, r"C:\Users\Admin\Documents\Projects\PhotoMangementSys\python")
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from PIL import Image

ROOT = r"C:\Users\Admin\Documents\Projects\PhotoMangementSys"
OUT = os.path.join(ROOT, ".visual")
appdata = os.path.join(os.environ["APPDATA"], "com.photomanagementsys.app")
key = open(os.path.join(appdata, "app.key"), "rb").read()
con = sqlite3.connect("file:" + os.path.join(appdata, "photos.db") + "?mode=ro", uri=True)
cfg = json.loads(con.execute("select value from app_settings where key='ai_studio:1'").fetchone()[0])
con.close()

def decrypt(enc):
    raw = base64.b64decode(enc[3:])
    return AESGCM(key).decrypt(raw[:12], raw[12:], None).decode()

from ai_studio.schemas import JobRequest, Providers, ProviderConfig
from ai_studio.workflows import style_transfer

req = JobRequest(
    tool="style_transfer",
    paths=[os.path.join(ROOT, "night_city.jpg")],
    prompt="",
    outputDir=os.path.join(OUT, "smoke_transfer_pro_out"),
    options={"mode": "transfer", "referencePath": os.path.join(ROOT, "covers", "login-sunset.jpg")},
    providers=Providers(
        vision=ProviderConfig(baseUrl=cfg["vision"]["base_url"], apiKey=decrypt(cfg["vision"]["api_key_enc"]), model=cfg["vision"]["model"]),
        edit=ProviderConfig(baseUrl=cfg["edit"]["base_url"], apiKey=decrypt(cfg["edit"]["api_key_enc"]), model="qwen-image-3.0-pro"),
    ),
)
res = style_transfer.run(req, lambda *a: None, None)
print(json.dumps(res, ensure_ascii=False))
for p in res["outputs"]:
    print("out", p, Image.open(p).size, os.path.getsize(p))
