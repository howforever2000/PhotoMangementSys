<script setup lang="ts">
/**
 * GlassFilters —— 液态玻璃的 SVG 滤镜定义（FEAT-108）
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么需要这个组件
 * ──────────────────────────────────────────────────────────────────────────
 * `backdrop-filter` 在 Chromium / WebView2 里**可以引用 SVG 滤镜**：
 *
 *     backdrop-filter: url(#liquid-lens) blur(6px);
 *
 * 这把"玻璃边缘把背后的东西弯折"真正做了出来 —— 而不只是模糊 + 假装有厚度。
 * 实测（.visual/probe-backdrop-svg.html + glass-refraction-lab.html）确认
 * 背景条纹确实被 feDisplacementMap 扭折了。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么滤镜定义要挂在应用根节点
 * ──────────────────────────────────────────────────────────────────────────
 * CSS 的 `url(#id)` 是**全文档作用域**的，但被引用的 <svg> 必须在
 * 同一个 document 里、且不能被 display:none（display:none 的 SVG 里的滤镜
 * 在部分浏览器实现里不会被解析）。所以：
 *   · 放在 App.vue 的根模板里（全应用只有一份，不随页面切换销毁）；
 *   · 用 width:0;height:0;position:absolute 把它"藏起来但不 display:none"。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 滤镜链为什么是这三步
 * ──────────────────────────────────────────────────────────────────────────
 * 参照 skill-liquid-glass（Android/Compose 的 backdrop 库）的光学模型：
 *   blur(8dp) + lens(24dp, 24dp) + chromaticAberration
 * Web 端能对应上的只有前半段：
 *
 *   ① feTurbulence  → 生成位移场（噪声）
 *   ② feGaussianBlur→ 把噪声柔化（不然位移是高频抖动的，边缘会出现锯齿噪点）
 *   ③ feDisplacementMap → 用位移场把背景按噪声方向偏移
 *
 * 真实玻璃的折射是**沿边缘方向**的（中心不动、边缘向内或向外偏），
 * 而噪声场是随机的 —— 这是 Web 方案与原生 RuntimeShader 的本质差距。
 * 缓解手段：baseFrequency 取很低（0.012）→ 位移场是大尺度缓变的，
 * 视觉上更接近"整块玻璃的厚度畸变"，而不是"磨砂玻璃的噪点"。
 *
 * ⚠️ 色散（chromaticAberration）没有做进生产：
 *   它需要拆 RGB 三通道分别位移再叠合（11 个滤镜图元，约 3 倍工作量），
 *   而在这个壁纸密度下肉眼收益极小 —— 已留在对照页里供评估，
 *   真要做时把 #liquid-lens-disperse 定义搬过来即可。
 */
</script>

<template>
  <svg class="glass-filters" width="0" height="0" aria-hidden="true" focusable="false">
    <defs>
      <!--
        液态玻璃边缘折射。
        scale=18：位移量的上限（px 量级）。对照页实测 16 与 18 视觉几乎无差，
        取 18 让 52px 高的顶栏上"边缘弯折"稍微明确一点。
        x/y -30%~160%：滤镜区域比元素本身外扩，否则边缘一圈会因为采样越界被裁掉，
        表现为"边缘没有折射、只有中间有"。
      -->
      <filter
        id="liquid-lens"
        x="-30%"
        y="-30%"
        width="160%"
        height="160%"
        color-interpolation-filters="sRGB"
      >
        <feTurbulence
          type="fractalNoise"
          baseFrequency="0.012 0.016"
          numOctaves="2"
          seed="7"
          result="noise"
        />
        <feGaussianBlur in="noise" stdDeviation="2" result="softNoise" />
        <feDisplacementMap
          in="SourceGraphic"
          in2="softNoise"
          scale="18"
          xChannelSelector="R"
          yChannelSelector="G"
        />
      </filter>
    </defs>
  </svg>
</template>

<style scoped>
/* 藏起来但**不能** display:none —— 否则部分实现不解析其中的滤镜 */
.glass-filters {
  position: absolute;
  width: 0;
  height: 0;
  overflow: hidden;
  pointer-events: none;
}
</style>
