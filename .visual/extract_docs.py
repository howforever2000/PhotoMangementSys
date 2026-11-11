import requests, re, html, sys

def get_text(u):
    r = requests.get(u, timeout=40, headers={"User-Agent": "Mozilla/5.0"})
    h = r.text
    h = re.sub(r"<script[\s\S]*?</script>", " ", h, flags=re.I)
    h = re.sub(r"<style[\s\S]*?</style>", " ", h, flags=re.I)
    h = re.sub(r"<[^>]+>", " ", h)
    h = html.unescape(h)
    h = re.sub(r"[ \t\r\f\v]+", " ", h)
    h = re.sub(r"\n\s*\n+", "\n", h)
    return h

pages = {
    "qwen-image-api": "https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-image-api",
    "chat-completions": "https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions",
}
for name, u in pages.items():
    t = get_text(u)
    print("#" * 100)
    print("PAGE:", name, "textlen", len(t))
    for kw in ["图像编辑", "modalities", "image_url", "qwen-image-3.0", "多图", "1:1", "size"]:
        start = 0
        cnt = 0
        while True:
            i = t.find(kw, start)
            if i < 0 or cnt >= 2:
                break
            seg = t[max(0, i - 500): i + 900]
            print(f"\n----- [{name}] kw={kw!r} @{i} -----")
            print(seg.strip())
            start = i + len(kw); cnt += 1
