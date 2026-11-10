"""桩测试：不依赖真实 fastapi/pydantic/requests，验证 AI 服务包能完整导入（FEAT-152）。

目的：在没有安装第三方依赖的机器上，仍能发现「导入期」的 NameError / 循环导入 /
路由注册错误。真实 HTTP 调用需在装有依赖的环境跑。
"""
import os
import sys
import types

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "python"))

# --- stub requests ---
req = types.ModuleType("requests")


class _Resp:
    status_code = 200
    text = ""

    def json(self):
        return {"data": [{"b64_json": ""}]}


req.post = lambda *a, **k: _Resp()
req.get = lambda *a, **k: _Resp()
sys.modules["requests"] = req

# --- stub fastapi ---
fastapi = types.ModuleType("fastapi")


class FastAPI:
    def __init__(self, *a, **k):
        pass

    def get(self, path):
        return lambda fn: fn

    def post(self, path):
        return lambda fn: fn


class HTTPException(Exception):
    def __init__(self, status_code=500, detail=None):
        super().__init__(detail)
        self.status_code = status_code
        self.detail = detail


fastapi.FastAPI = FastAPI
fastapi.HTTPException = HTTPException
sys.modules["fastapi"] = fastapi

# --- stub uvicorn ---
uv = types.ModuleType("uvicorn")
uv.run = lambda *a, **k: None
sys.modules["uvicorn"] = uv

# --- stub pydantic ---
pyd = types.ModuleType("pydantic")


class BaseModel:
    def __init__(self, **kw):
        self.__dict__.update(kw)


def Field(default=None, default_factory=None, **k):
    if default_factory is not None:
        return default_factory()
    return default


pyd.BaseModel = BaseModel
pyd.Field = Field
sys.modules["pydantic"] = pyd

import importlib  # noqa: E402

server = importlib.import_module("ai_studio.server")
jobs = importlib.import_module("ai_studio.jobs")
wf = importlib.import_module("ai_studio.workflows")
presets = importlib.import_module("ai_studio.presets")
compose = importlib.import_module("ai_studio.compose")

assert set(wf.REGISTRY.keys()) == {"journal", "beauty", "style_transfer", "upscale"}
assert "blank" in presets.JOURNAL_TEMPLATES
assert callable(compose.export_journal)
print("AI STUDIO IMPORT OK; tools =", sorted(wf.REGISTRY))
