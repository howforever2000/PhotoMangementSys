"""AI 创作微服务 · PyInstaller 打包入口（FEAT-152 / 打包修复）

为什么单独放一个顶层启动器而不直接把 `ai_studio/server.py` 当入口：
    server.py 内部用的是**相对导入**（`from . import config, jobs`）。当它被
    PyInstaller 当作「顶层脚本」执行时 `__package__` 为 None，相对导入会直接
    抛 ImportError。这里以「导入包模块」的方式启动，保留正确的包上下文。

开发期用 `python -m ai_studio.server`，打包版用本文件 → `python/ai-studio.spec`。
"""
from ai_studio.server import main

if __name__ == "__main__":
    main()
