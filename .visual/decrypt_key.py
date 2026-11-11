import os, sqlite3, json, base64, sys

appdata = os.path.join(os.environ["APPDATA"], "com.photomanagementsys.app")
keypath = os.path.join(appdata, "app.key")
print("app.key exists:", os.path.isfile(keypath), "appdata:", appdata)
if os.path.isfile(keypath):
    kb = open(keypath, "rb").read()
    print("app.key len:", len(kb))

try:
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    HAVE = True
except Exception as e:
    HAVE = False
    print("cryptography missing:", e)

if HAVE and os.path.isfile(keypath):
    key = kb
    db = os.path.join(appdata, "photos.db")
    con = sqlite3.connect("file:" + db + "?mode=ro", uri=True)
    row = con.execute("select value from app_settings where key='ai_studio:1'").fetchone()
    con.close()
    cfg = json.loads(row[0])
    for kind in ("vision", "edit"):
        enc = (cfg.get(kind) or {}).get("api_key_enc", "")
        print(f"--- {kind}: enc_len={len(enc)} prefix={enc[:3]}")
        if enc.startswith("v1:"):
            raw = base64.b64decode(enc[3:])
            nonce, ct = raw[:12], raw[12:]
            try:
                pt = AESGCM(key).decrypt(nonce, ct, None).decode()
                print(f"    plaintext_len={len(pt)} head={pt[:6]} tail={pt[-4:]}")
            except Exception as e:
                print("    decrypt FAILED:", e)
