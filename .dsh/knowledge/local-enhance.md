# 本地增强（胶片 / 参考图调色 / 超分+人脸）踩坑与做法

> 适用：创意工坊「本地增强」三工具（FEAT-163）及任何纯本机 LUT / ONNX 图像处理。

## 1. `.cube` 3D LUT 的轴序（触发：手写 LUT 解析/套用）
- 先做：Adobe `.cube` 数据按「红变化最快」排列，扁平索引 `k = r + N*g + N^2*b`，`reshape(n,n,n,3)` 后数组轴序是 **[b,g,r]**；取样写 `lut[b, g, r]`，三线性用 floor/ceil 8 角插值。
- 为什么：按 [r,g,b] 直接 reshape 会通道错位（红蓝互换），但图像仍「看起来有色」，难肉眼定位。
- 违规症状：套 LUT 后颜色怪异但对比正常；用纯色 probe（如 (255,0,0)）能立刻验证方向。

## 2. SCRFD 人脸检测需要周边上下文（触发：直接拿紧裁 512 人脸测检测）
- 先做：紧裁的 512×512 对齐人脸（如 GFPGAN 官方 cropped_faces）在 det_10g 上最高分仅 ~0.336（<0.5 不检出）；给四周补灰边到 1.8× 后同脸得分 ~0.88 稳定检出。真实照片有人脸上下文，正常可检出。
- 为什么：SCRFD 训练分布是「场景图里找人脸」，无边界/头部轮廓的贴边人脸偏离分布。
- 违规症状：单测用人脸裁图 → 「未检测到人脸」误判为检测器坏了；用带上下文图复测即可排除。

## 3. 本机 ONNX 推理（触发：在 ai_studio 里加本地模型）
- 先做：`onnxruntime` 惰性 import；providers 优先 `DmlExecutionProvider`（需装 onnxruntime-directml），失败回退 `CPUExecutionProvider`；会话按模型路径缓存复用；Real-ESRGAN 大图分块（256/512 tile）防 OOM；GFPGAN 输入固定 [1,3,512,512] 且归一化到 [-1,1]。
- 为什么：直接整图 4× 在轻薄本会内存峰值过高；每次新建 session 加载几百 MB 模型很慢。
- 实测：CPU provider 下 200×123→4× 约 6.8s、GFPGAN 512 单脸约 3.1s（可接受，DirectML 会更快）。

## 4. 本环境联网/下载（触发：探测 GitHub / 下载模型）
- 先做：PowerShell `Invoke-RestMethod`/`Invoke-WebRequest` 直连外网常被阻断（「基础连接已经关闭」）；改用 `web_fetch` 或 python `urllib` + Range 续传 + 重试。GitHub raw/releases 可达；huggingface 直连常超时。
- 违规症状：pwsh 抓 github api 直接失败；python urllib 偶发 ConnectionResetError(WinError 10054)，需重试。
