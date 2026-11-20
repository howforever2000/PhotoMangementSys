"""预设与提示词模板（FEAT-152）

**唯一的差异来源**：四个工具复用同一套「读图 → 风格化 → 合成 → 导出」步骤，
区别只在 preset（版式/风格/级别）与提示词模板。因此预设集中放这里，
工作流只做「取预设 → 拼提示词 → 调 provider」，不各写一套。

提示词用英文：主流图像模型对英文风格指令更稳定；中文只用于「配文」这类
交给视觉模型理解后直接上屏的文本。
"""

from . import catalog, modelstyle

# ---------------- 全局硬约束：结构保全（FEAT-169） ----------------
# 用户规则：「滤镜迁移不能改变每个物体的形状，包括人物、主体、背景；人物美颜只做
# 分区磨皮与细节」。云端图生图是**整幅重绘**，提示词只能做软约束 —— 所以把它写成
# 不可省略的硬段，拼进每个「保真类」提示词（滤镜 / 美颜 / 超分 / 反思微调），
# 与之配套的还有 `critic.structure_compare` 的出图后校验（软约束 + 硬验收双保险）。
#
# 为什么反复强调「像素 / 轮廓 / 比例」：这类网关对抽象的「不要变形」几乎无感，
# 但对具体的「不要移动、不要重塑、不要重新构图、不要改脸型体型」响应明显更好。
# 为什么**不**拼进 journal：素描 / 水彩 / 水墨 / 卡通形象都是生成式再创作，改形状是它的目的本身。
STRUCTURE_LOCK = (
    "STRICT STRUCTURAL PRESERVATION — this is a color/lighting-only edit. "
    "Do not move, resize, redraw, reshape, warp, stretch, add or remove any object. "
    "Every person, every face, every body, every prop, every building and the entire "
    "background must keep the exact same outline, silhouette, position, pose, proportions "
    "and framing as the input image. Do not change body shape, waistline, face shape, jawline, "
    "eye size, nose shape or limb thickness. Do not reframe, crop, rotate, straighten, "
    "recompose or change the camera angle. Do not add or remove elements. "
    "Only shift color, white balance, tone curve, contrast, saturation, grain and light; "
    "keep every edge where it already is. The output must register pixel-for-pixel with the input."
)


def structure_lock() -> str:
    """结构保全硬段（供工作流拼提示词、供前端展示同一份措辞）。"""
    return STRUCTURE_LOCK


STRUCTURE_LOCK_ZH = "颜色与质感可以变，几何形状一律不许变。"


def structure_lock_zh() -> str:
    """结构保全的中文表述（界面文案与英文硬段同一份语义）。"""
    return STRUCTURE_LOCK_ZH


def lock(prompt: str) -> str:
    """把硬段追加到任意保真类提示词末尾（幂等：已含则不再加）。"""
    base = prompt.strip()
    if not base:
        return STRUCTURE_LOCK
    if STRUCTURE_LOCK in base:
        return base
    return f"{base} {STRUCTURE_LOCK}"

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
# style → (label, group, 风格化提示词)。要求 no text：文字由版式层渲染。
#
# `group` 只影响前端分组呈现（`src/utils/aiPresets.ts` 的 JOURNAL_STYLE_GROUPS），
# 后端不按组做任何分支 —— 分组是排版诉求，不是处理差异。
#
# 卡通组（FEAT-170）为什么**不点名**具体工作室/作品：主流图生图网关对 IP 名有
# 关键词审核，直接写工作室名会随机拒答或降质；改用「风格特征描述」既能稳定出图，
# 也避免把第三方 IP 写进产品文案。中文标签保留用户熟悉的说法即可。
JOURNAL_STYLES: dict[str, dict] = {
    "sketch": {
        "label": "素描",
        "group": "drawing",
        "prompt": (
            "Transform this photo into a clean hand-drawn pencil sketch illustration, "
            "monochrome graphite line work with soft shading on white paper, keep the main "
            "subject clearly recognizable, elegant and minimal, no text, no watermark."
        ),
    },
    "watercolor": {
        "label": "水彩",
        "group": "drawing",
        "prompt": (
            "Repaint this photo as a soft watercolor illustration, gentle translucent washes, "
            "warm paper texture, light and airy, keep the main subject recognizable, no text."
        ),
    },
    "ink": {
        "label": "水墨",
        "group": "drawing",
        "prompt": (
            "Repaint this photo in traditional Chinese ink-wash style, expressive brush strokes, "
            "elegant negative space, muted palette, keep the subject recognizable, no text."
        ),
    },
    "film": {
        "label": "胶片",
        "group": "drawing",
        "prompt": (
            "Give this photo a warm analog film look, gentle grain and soft highlights, "
            "cinematic muted tones, keep composition unchanged, no text."
        ),
    },
    "anime_summer": {
        "label": "动画夏日",
        "group": "cartoon",
        # 提示词结构按社区验证过的「分层叠加」写（媒介 → 人物上色 → 背景笔触 → 调色 →
        # 光线 → 情绪 → 约束），而不是一串形容词。三条来自实测的经验：
        #   1) 「in the style of XXX」这类锚点词几乎无效  -  -  模型只会给一个风格平均值，
        #      所以这里全程描述**画面怎么画**，不写「谁的风格」。
        #   2) 人物与背景必须是**两套技法**：人物赛璐璐平涂（cel shading），背景水彩晕染
        #      （watercolor background） -  -  这是 2000 年代手绘动画最核心的辨识度来源。
        #   3) 网关没有 negative prompt 通道（openai_compat 只传 prompt 一个字符串），
        #      所以「不要 3D / 不要写实」必须写成正面约束句，并重复一次以提权重。
        "prompt": (
            # ① 媒介：一句话锁定「手绘 2D 动画电影截帧」，并当场排除 3D 与实拍
            "Repaint this photo as a hand-painted 2D animated film still  -  a single frame "
            "from a 2000s theatrical animation, drawn and painted by hand, not 3D, "
            "not CGI, not photorealistic. "
            # ② 人物：赛璐璐平涂 + 细墨线，明确「只在人物上有线稿」
            "Characters are rendered with flat cel shading: crisp two-tone shadow shapes, "
            "smooth untextured color fills, no gradient banding and no plastic 3D specular "
            "highlights. Thin clean ink outlines trace the characters only, never the scenery. "
            "Faces are simplified and expressive, eyes drawn with a few confident strokes, "
            "keeping the person's recognizable features, age and expression. "
            # ③ 背景：水彩晕染，与人物形成技法对比
            "Backgrounds are lush hand-painted watercolor: layered translucent washes, "
            "visible paper grain, soft bleeding edges, individually painted leaves and grass "
            "blades, dense natural detail in the foliage. Depth comes from painted color and "
            "atmosphere, not from depth of field or lens blur. "
            # ④ 调色：盛夏高饱和但干净
            "Palette is a rich summer palette: deep saturated greens in the foliage, "
            "a vivid cyan-blue sky, towering bright white cumulus clouds with soft grey-violet "
            "shadow cores, warm golden sunlight, and small clean accents of red and yellow. "
            # ⑤ 光线：午后斜阳 + 空气感
            "Lighting is warm late-afternoon sun raking in from the side, long soft shadows, "
            "gentle sun haze and airborne light, dappled light through leaves, a soft warm "
            "bloom around highlights. No hard rim light, no volumetric god rays, no lens flare. "
            # ⑥ 情绪
            "Mood is nostalgic, quiet and quietly joyful  -  the feeling of a long summer "
            "afternoon. Keep the subject, its pose and the overall composition recognizable. "
            # ⑦ 约束：文字交给版式层
            "No text, no lettering, no watermark, no signature."
        ),
    },
    "guoman_3d": {
        "label": "国漫 3D",
        "group": "cartoon",
        # 与卡通 2D 相反，这一款要**明确排除手绘感**：社区里 3D 国漫最容易翻的车是
        # 「廉价手办味 / 塑料感」，解法是把材质与布光写成工业级参数：
        #   SSS 皮肤 → Rembrandt 主光 + rim light 勾边 → PBR 材质分件 → 体积光与 AO。
        # 同样受限于没有 negative prompt，用「smooth soft shading, not plastic」正面表述。
        "prompt": (
            # ① 媒介：一句话锁定「高预算三维动画电影截帧」
            "Repaint this photo as a cinematic frame from a high-budget Chinese 3D animated "
            "feature film  -  a polished CG render with stylized proportions, "
            "not a photograph, not a hand-drawn illustration. "
            # ② 皮肤：SSS 是这一款与「塑料手办感」的分水岭
            "Skin uses subsurface scattering: soft translucent depth, a subtle warm red glow "
            "where light grazes ears, nose and fingertips, even pore-level micro detail, "
            "smooth soft shading with no plastic sheen and no waxy flatness. "
            "Facial features are refined and appealing, eyes rendered as glossy cel-shaded "
            "anime-style eyes with a clean specular highlight, keeping the person's "
            "recognizable features, age and expression. "
            # ③ 毛发与材质：分件描述
            "Hair is groomed strand by strand, each lock separated with a soft highlight and "
            "natural flyaways. Clothing and props are physically based: fabric with visible "
            "weave and believable folds, metal with cool hard reflections, leather with "
            "grain, jade and ceramic with soft translucency. "
            # ④ 布光：工业级三灯逻辑，这是立体感的来源
            "Lighting is film-grade: a warm Rembrandt key light from forty-five degrees above "
            "and to one side, a cool rim light separating the subject from the background, "
            "and soft ambient fill. Volumetric light shafts, atmospheric haze, "
            "ambient occlusion in the crevices, gentle bloom on the highlights. "
            # ⑤ 镜头
            "Shot on a long lens with shallow depth of field, background falling into a soft "
            "creamy bokeh, subtle chromatic edge and a faint anamorphic flare. "
            # ⑥ 调色：国漫标志性的青金对撞
            "Color grading is rich and tasteful: teal shadows against warm gold highlights, "
            "saturated but controlled, high dynamic range with clean rolloff, "
            "deep contrast in the shadows and luminous skin tones. "
            # ⑦ 氛围与约束
            "Mood is epic yet warm, elegant and cinematic. Keep the subject, its pose and the "
            "overall composition recognizable. "
            "No text, no lettering, no watermark, no signature."
        ),
    },
}

# ---------------- 人物美颜 ----------------
# style → (label, 提示词基句)；level → (label, 强度句)
#
# FEAT-169 收紧：用户规则要求「美颜保持当前人体形状不变，只做分区磨皮和细节」。
# 旧措辞里 "clear eyes" 会被模型理解成放大眼睛，属于改形状，已删；
# 统一改成「只处理皮肤质感 / 瑕疵 / 肤色 / 光线，脸型与体型一律不动」。
BEAUTY_STYLES: dict[str, dict] = {
    "natural": {
        "label": "自然",
        "prompt": (
            "Professional portrait skin retouching only: smooth skin texture, remove blemishes "
            "and temporary spots, even out skin tone, keep pores and real skin texture alive, "
            "keep every facial feature exactly as it is."
        ),
    },
    "crisp": {
        "label": "清透",
        "prompt": (
            "Bright clean portrait skin retouch: clean even skin tone, soft flattering light, "
            "fresh luminous skin, remove dullness and small blemishes only, "
            "keep every facial feature exactly as it is."
        ),
    },
    "film": {
        "label": "胶片",
        "prompt": (
            "Film-look portrait skin retouch: warm cinematic tones, soft highlights, gentle grain, "
            "natural matte skin with real texture, keep every facial feature exactly as it is."
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
    """人物美颜提示词：只做皮肤质感与细节，**永远**追加结构保全硬段。"""
    base = BEAUTY_STYLES.get(style, BEAUTY_STYLES["natural"])["prompt"]
    lv = BEAUTY_LEVELS.get(level, BEAUTY_LEVELS["medium"])["prompt"]
    extra = (extra or "").strip()
    return lock(" ".join(x for x in [base, lv, extra] if x))


def upscale_prompt(scale: int) -> str:
    return lock(UPSCALE_PROMPT.format(scale=scale))


def transfer_prompt(ref_desc: str = "", extra: str = "") -> str:
    """滤镜迁移提示词（仅参考图来源）：参考图描述 + 补充要求，末尾拼结构保全硬段。"""
    base = STYLE_TRANSFER_PROMPT
    ref = (ref_desc or "").strip()
    if ref:
        base += f" Reference style: {ref}."
    tail = (extra or "").strip()
    if tail:
        base += f" {tail}"
    return lock(base)


def auto_grade_prompt(extra: str = "") -> str:
    """自主调色提示词：通用调色描述 + 补充要求，末尾拼结构保全硬段。"""
    base = AUTOGRADE_BASE_PROMPT
    tail = (extra or "").strip()
    if tail:
        base += f" {tail}"
    return lock(base)


# ---------------- 智能修图：规划 / 复核 / 微调（FEAT-152 扩展） ----------------
# 规划器可用算子目录：告诉模型有哪些白名单算子与参数，避免它凭空发明算子。
# 实际强约束在 pipeline.validate_plan（白名单 + 参数夹取 + 顺序规范化），此文本只是「告知」。
#
# 算子表由 `op_catalog.json` 生成（唯一真相源），规划提示词只取其中的云端子集
# （`CLOUD_OP_CATALOG`）；完整表留给配方校验与「创意组合」的前端使用。
# 「模型理解」路线只编排云端算子（FEAT-197）：本机算子属于「创意组合」那条路线，
# 混进规划会让「按模型计费」的口径失真，也会把一句话出片变成本机长等待。
CLOUD_OP_CATALOG = catalog.op_catalog_prompt(only=catalog.cloud_ops())


def planner_prompt(
    user_intent: str,
    strength: float | None = None,
    style_id: str = "",
) -> str:
    """给规划模型（多模态）的提示：看意图 + 照片，产出受约束 JSON 计划。

    只列云端算子：这条链路的产物是「原图 → 理解 → 文生图 → 1K 出图」，
    全程按模型计费；能跑在本机的调色/胶片/形变/磨皮请走「创意组合」。

    `strength`（0~1，可缺省）是界面上的「强度」滑杆。**必须真的进提示词**：
    前端传了而后端不读，就是绑定却不生效的死控件（FEAT-168 / BUG-2026-1118-003
    的同类教训）。它表达的是「改动幅度偏好」，不是某个算子的参数。

    `style_id`（FEAT-198）只影响**规划方向**：告诉模型「用户挑了一款影调风格，
    应该走调色/滤镜迁移这条线」。真正的风格段落由算子层 `with_style()` 确定性追加，
    不依赖模型把它抄进 `extra` —— 抄漏就静默失效（见 `ai_studio/modelstyle.py`）。
    """
    intent = (user_intent or "").strip() or "自然、得体地优化这张照片"
    amount = ""
    if strength is not None:
        try:
            s = max(0.0, min(1.0, float(strength)))
        except (TypeError, ValueError):
            s = 0.64
        if s < 0.34:
            amount = "改动幅度：保守，尽量贴近原片，不要明显的风格化。\n"
        elif s < 0.67:
            amount = "改动幅度：适中，能看出修过但保持自然。\n"
        else:
            amount = "改动幅度：明显，风格与影调可以更强烈，但不得改变构图与形状。\n"
    picked = modelstyle.label_of(style_id)
    style_hint = (
        f"用户还挑了一款影调风格「{picked}」（id={style_id}），"
        "请把调色/滤镜迁移排在计划里（风格段落由执行层自动追加，你不需要复述它）。\n"
        if picked
        else ""
    )
    style_list = modelstyle.prompt_lines()
    return (
        "你是照片后期编排助手。根据用户意图和这张照片，规划一个线性、可复现的修图步骤序列。\n"
        f"{CLOUD_OP_CATALOG}\n"
        f"可选影调风格（用户可在界面里挑一款，你没挑到时不要自己编）：\n{style_list}\n"
        f"用户意图：{intent}\n"
        f"{amount}"
        f"{style_hint}"
        "注意：步骤必须按上面标注的前置顺序排列；最多 4 步，只排真正必要的步骤。\n"
        "重要：改形状是例外而不是默认。除非用户明确要求改变脸型，否则不要编排改变几何的步骤 —— "
        "调色、滤镜、美颜都只改颜色与质感。\n"
        "只输出 JSON，不要解释，格式："
        '{"intent":"简述意图","steps":[{"op":"style_transfer","params":{}}]}'
    )


def with_style(extra: str, style_id: str) -> str:
    """把风格段落追加到 `extra` 末尾（FEAT-198）。

    **注意别在链路里用它拼 `params.extra`**：`catalog.norm_params` 对 string 参数
    截到 500 字，而一条风格描述（中文原文 + 英文要点）常到 600~1300 字 —— 塞进去会
    被静默截断。真正生效的注入点是 `ops._run_edit` 读作业级 `RunContext.style_prompt`
    （不经过参数夹取）。本函数留给不需要过 `norm_params` 的调用方。
    """
    base = (extra or "").strip()
    block = modelstyle.compose(style_id)
    if not block:
        return base
    return f"{base} {block}".strip() if base else block


def verifier_prompt(intent: str, extra: str = "") -> str:
    """给复核模型（多模态）的提示：对照意图判断结果是否达标，只输出 {pass,issues}。"""
    intent = (intent or "").strip() or "自然、得体地优化这张照片"
    tail = f" 额外要求：{extra.strip()}。" if (extra or "").strip() else ""
    return (
        "你是严格的修图质量审核员。这是修图结果，请对照用户意图判断是否已经达标。"
        "只关注是否满足意图，以及是否有明显失真、畸形、文字乱码、构图或主体被破坏。\n"
        f"用户意图：{intent}。{tail}\n"
        '只输出 JSON，不要解释：{"pass": true/false, "issues": ["问题1","问题2"]}；'
        "pass 为 true 时 issues 留空。"
    )


def refine_prompt(issues, extra: str = "") -> str:
    """给图像编辑模型的微调提示：按复核问题点重绘，保持身份/构图。

    `lock()` 在此处是**必须**的：反思微调是整幅重绘，最容易在「修问题」的同时
    顺手把脸型、体型、构图挪一点 —— 正是用户明令禁止的那类改动。
    """
    items = [str(x).strip() for x in (issues or []) if str(x).strip()]
    lines = "；".join(f"（{i + 1}）{t}" for i, t in enumerate(items)) or "整体再自然、干净一些"
    tail = f" 若无冲突可兼顾：{extra.strip()}。" if (extra or "").strip() else ""
    return lock(
        f"在保持原始主体身份、构图与真实感的前提下，修正这张图上的以下问题：{lines}。"
        f"不要改变人物长相，不要新增文字或水印，输出修好的整张图。{tail}"
    )
