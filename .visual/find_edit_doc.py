import requests, re

def html(u):
    return requests.get(u, timeout=40, headers={"User-Agent": "Mozilla/5.0"}).text

h = html("https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-image-api")
hrefs = set(re.findall(r'href="(/zh/model-studio/[^"#?]+)"', h))
cand = sorted([x for x in hrefs if ("edit" in x.lower() or "image" in x.lower())])
print("IMAGE/EDIT candidate doc paths:")
for x in cand:
    print("  ", x)
