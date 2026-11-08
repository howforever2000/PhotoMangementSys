"""模型通道：契约、OpenAI 兼容客户端、档位映射、端点探针。

## 为什么只有一种协议

用户决定「不预设 provider，自己填地址和 Key」（docs/v5.md §25 修订）。
现实是绝大多数自填地址都是 **OpenAI 兼容**：APIMart/Seedream、SiliconFlow、
OpenRouter、以及本地 llama.cpp / Ollama / LM Studio / ComfyUI 的 `/v1`。
所以这里只实现一种协议，**一份客户端代码同时服务图像与规划**。

好处不只是省代码：探针测的就是真实调用会走的那段代码，
"测试通过但跑不通"这类问题从结构上消失。

混元原生 Chat&Messages 只留接口位（`providers/native_stub.py` 不实现），
等真有需求再补 —— 现在补等于维护一条没人验证的分支。
"""
