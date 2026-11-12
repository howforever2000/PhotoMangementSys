import json, urllib.request

def get(url):
    req = urllib.request.Request(url, headers={"User-Agent": "probe", "Accept": "application/vnd.github+json"})
    with urllib.request.urlopen(req, timeout=40) as r:
        return json.load(r)

print("== facefusion-assets releases: esrgan/gfpgan/codeformer assets ==")
try:
    rels = get("https://api.github.com/repos/facefusion/facefusion-assets/releases?per_page=100")
    for rel in rels:
        tag = rel.get("tag_name")
        for a in rel.get("assets", []):
            n = a["name"].lower()
            if ("esrgan" in n or "gfpgan" in n or "codeformer" in n):
                print(f"{tag:>16} | {a['name']:<32} | {a['size']/1e6:7.1f}MB | {a['browser_download_url']}")
except Exception as e:
    print("ERR releases", type(e).__name__, str(e)[:150])

print("\n== github repo search: real-esrgan onnx ==")
for q in ["real-esrgan+onnx", "realesrgan+onnx+model", "gfpgan+onnx"]:
    try:
        d = get(f"https://api.github.com/search/repositories?q={q}&sort=stars&per_page=6")
        print(f"-- q={q} total={d.get('total_count')}")
        for it in d.get("items", []):
            print(f"   {it['full_name']:<45} {it['stargazers_count']:>6}st  {it.get('license') and it['license'].get('spdx_id')}  {(it.get('description') or '')[:70]}")
    except Exception as e:
        print("ERR search", q, type(e).__name__, str(e)[:120])
