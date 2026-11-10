"""预设与提示词模板（FEAT-152）

**唯一的差异来源**：四个工具复用同一套「读图 → 风格化 → 合成 → 导出」步骤，
区别只在 preset（版式/风格/级别）与提示词模板。因此预设集中放这里，
工作流只做「取预设 → 拼提示词 → 调 provider」，不各写一套。

提示词用英文：主流图像模型对英文风格指令更稳定；中文只用于「配文」这类
交给视觉模型理解后直接上屏的文本。
"""

# ---------------- 主题手账：版式模板 ----------------
# 每个模板是一组**确定性排版参数**（Pillow 直接按参数画），不做「让模型生成整页」，
# 因为中文排版交给图像模型几乎必然乱码/错字（项目既有结论，见 Workshop.vue 注释）。
JOURNAL_TEMPLATES: dict[str, dict] = {
    "blank": {
        "label": "留白",
        "page": (1240, 1754),          # A4 @150dpi
        "bg": (252, 251, 248),
        "margin": 118,
        "title_size": 52,
        "caption_size": 34,
        "title_color": (38, 36, 33),
        "caption_color": (62, 60, 56),
        "accent": (178, 142, 83),
        "image_ratio": 0.62,           # 图片高度占页高比例
        "frame": False,
        "dark": False,
    },
    "magazine": {
        "label": "杂志",
        "page": (1240, 1754),
        "bg": (248, 247, 244),
        "margin": 0,
        "title_size": 58,
        "caption_size": 33,
        "title_color": (26, 25, 23),
        "caption_color": (58, 57, 53),
        "accent": (198, 92, 66),
        "image_ratio": 0.70,           # 上半幅满版
        "frame": False,
        "dark": False,
    },
    "film": {
        "label": "胶片",
        "page": (1240, 1754),
        "bg": (26, 27, 31),
        "margin": 96,
        "title_size": 50,
        "caption_size": 32,
        "title_color": (238, 235, 228),
        "caption_color": (206, 202, 194),
        "accent": (214, 176, 106),
        "image_ratio": 0.60,
        "frame": True,
        "dark": True,
    },
}

# ---------------- 主题手账：图像风格 ----------------
# style → (label, 风格化提示词)。要求 no text：文字由版式层渲染。
JOURNAL_STYLES: dict[str, dict] = {
    "sketch": {
        "label": "素描",
        "prompt": (
            "Transform this photo into a clean hand-drawn pencil sketch illustration, "
            "monochrome graphite line work with soft shading on white paper, keep the main "
            "subject clearly recognizable, elegant and minimal, no text, no watermark."
        ),
    },
    "watercolor": {
        "label": "水彩",
        "prompt": (
            "Repaint this photo as a soft watercolor illustration, gentle translucent washes, "
            "warm paper texture, light and airy, keep the main subject recognizable, no text."
        ),
    },
    "ink": {
        "label": "水墨",
        "prompt": (
            "Repaint this photo in traditional Chinese ink-wash style, expressive brush strokes, "
            "elegant negative space, muted palette, keep the subject recognizable, no text."
        ),
    },
    "film": {
        "label": "胶片",
        "prompt": (
            "Give this photo a warm analog film look, gentle grain and soft highlights, "
            "cinematic muted tones, keep composition unchanged, no text."
        ),
    },
}

# ---------------- 人物美颜 ----------------
# style → (label, 提示词基句)；level → (label, 强度句)
BEAUTY_STYLES: dict[str, dict] = {
    "natural": {
        "label": "自然",
        "prompt": (
            "Professional portrait retouching: smooth skin while preserving natural skin "
            "texture and real facial features, subtle blemish removal, keep identity unchanged."
        ),
    },
    "crisp": {
        "label": "清透",
        "prompt": (
            "Bright and clean portrait retouch: even skin tone, soft flattering light, "
            "fresh youthful look, clear eyes, keep identity unchanged."
        ),
    },
    "film": {
        "label": "胶片",
        "prompt": (
            "Film-look portrait retouch: warm cinematic tones, soft highlights, gentle grain, "
            "natural skin, keep identity unchanged."
        ),
    },
}

BEAUTY_LEVELS: dict[str, dict] = {
    "light": {"label": "轻", "prompt": "Apply at low intensity (about 25%), very subtle and natural."},
    "medium": {"label": "中", "prompt": "Apply at moderate intensity (about 50%), clearly improved but natural."},
    "strong": {"label": "强", "prompt": "Apply at strong intensity (about 80%), polished yet still natural and lifelike."},
}

# ---------------- 滤镜迁移 / 自主调色 ----------------
STYLE_TRANSFER_PROMPT = (
    "Apply the color grading, tonal mood and lighting style of the reference image to this "
    "photo. Preserve the original subject and composition, keep it photorealistic, "
    "no text, no watermark."
)
AUTOGRADE_BASE_PROMPT = (
    "Professionally color grade this photo, cinematic and pleasing, keep the subject and "
    "composition unchanged, no text."
)

# ---------------- 超分 ----------------
UPSCALE_PROMPT = (
    "Upscale and enhance this photo to higher resolution ({scale}x), sharpen fine details, "
    "reduce noise and compression artifacts, keep colors and composition faithful, no text."
)


def journal_style_prompt(style: str, theme: str) -> str:
    """拼手账风格化提示词：风格基句 + 主题意象。"""
    base = JOURNAL_STYLES.get(style, JOURNAL_STYLES["sketch"])["prompt"]
    theme = (theme or "").strip()
    if theme:
        return f"{base} The illustration should convey the theme: {theme}."
    return base


def beauty_prompt(style: str, level: str, extra: str = "") -> str:
    base = BEAUTY_STYLES.get(style, BEAUTY_STYLES["natural"])["prompt"]
    lv = BEAUTY_LEVELS.get(level, BEAUTY_LEVELS["medium"])["prompt"]
    extra = (extra or "").strip()
    return " ".join(x for x in [base, lv, extra] if x)


def upscale_prompt(scale: int) -> str:
    return UPSCALE_PROMPT.format(scale=scale)
