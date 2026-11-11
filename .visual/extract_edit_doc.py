import requests, re, html

def get_text(u):
    h = requests.get(u, timeout=40, headers={"User-Agent": "Mozilla/5.0"}).text
    h = re.sub(r"<script[\s\S]*?</script>", " ", h, flags=re.I)
    h = re.sub(r"<style[\s\S]*?</style>", " ", h, flags=re.I)
    h = re.sub(r"<[^>]+>", " ", h)
    h = html.unescape(h)
    h = re.sub(r"[ \t\r\f\v]+", " ", h)
    h = re.sub(r"\n\s*\n+", "\n", h)
    return h

u = "https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-image-edit-api"
t = get_text(u)
print("textlen", len(t))
# find the main content start (after nav ~ "千问-图像编辑")
start = t.find("千问-图像编辑")
# print a big window of the actual body
for kw in ["模型概览", "qwen-image-3.0", "input", "size", "content", "同步接口", "图像编辑", "多图", "curl"]:
    i = t.find(kw)
    print(f"\n### kw={kw!r} @{i}")
    if i >= 0:
        print(t[i-200:i+1200])
