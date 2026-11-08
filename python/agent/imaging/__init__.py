"""图像算法层（Pillow only）。

为什么这一层放在 Python（docs/v5.md §0.1）：编码、合成排版、LUT 类滤镜在
Pillow/libjpeg-turbo 上比 Rust `image` 快且代码量小一个数量级；
已经跑得好的（DCT 缩略图、批量扫描、向量扫描）继续留在 Rust，不重复实现。

**不引入 numpy / opencv**：这是 exe 体积预算（≤40MB）的硬约束。
所有像素处理走 Pillow 的 LUT / 通道运算 / ImageFilter。
"""
