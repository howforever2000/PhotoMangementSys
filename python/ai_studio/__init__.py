"""AI 创作微服务（FEAT-152）——包入口。

分层（与 VCR 同构，见 server.py 顶部说明）：
  接口层  server.py（FastAPI 路由 + DTO 薄壳）
  工作流  workflows/（journal / beauty / style_transfer / upscale）
  步骤    providers/（OpenAI 兼容调用）+ compose（Pillow 版式与 PDF）
  预设    presets.py（风格/级别/模板，只影响提示词与版式参数）
"""
