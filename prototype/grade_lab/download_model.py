import sys, os, time, urllib.request

url, dest = sys.argv[1], sys.argv[2]
os.makedirs(os.path.dirname(dest) or ".", exist_ok=True)
MAX_TRIES = 80
last_pct = -1
for attempt in range(MAX_TRIES):
    have = os.path.getsize(dest) if os.path.exists(dest) else 0
    headers = {"User-Agent": "dl", "Range": f"bytes={have}-"}
    try:
        req = urllib.request.Request(url, headers=headers)
        with urllib.request.urlopen(req, timeout=60) as r:
            cr = r.headers.get("Content-Range")
            total = None
            if cr and "/" in cr:
                total = int(cr.split("/")[1])
            elif r.headers.get("Content-Length"):
                total = have + int(r.headers["Content-Length"])
            mode = "ab" if (have > 0 and r.status == 206) else "wb"
            if mode == "wb":
                have = 0
            with open(dest, mode) as f:
                while True:
                    chunk = r.read(1 << 20)
                    if not chunk:
                        break
                    f.write(chunk)
                    have += len(chunk)
                    if total:
                        pct = int(have * 100 / total)
                        if pct // 5 != last_pct // 5:
                            print(f"{pct}% {have/1e6:.1f}/{total/1e6:.1f}MB", flush=True)
                            last_pct = pct
        if total and have >= total:
            print("DONE", dest, have, flush=True)
            break
        if not total:
            print("DONE(no-total)", dest, have, flush=True)
            break
        print(f"incomplete {have}/{total}, retry", flush=True)
    except Exception as e:
        print(f"retry#{attempt} {type(e).__name__} {str(e)[:110]}", flush=True)
        time.sleep(2)
else:
    print("FAILED", dest, flush=True)
