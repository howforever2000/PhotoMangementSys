<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import type { EditHistogram } from "../../../types/edit";
import { useThemeStore } from "../../../stores/theme";

/**
 * 直方图面板（FEAT-099）
 * ===================================================================
 * 画三通道直方图 + 亮度直方图（Lightroom 面板的经典形态）：
 *
 * - RGB 三色半透明叠加 + 白色亮度包络；
 * - 两侧显示截断计数（`clippedLow` / `clippedHigh`），点了高亮对应通道 ——
 *   这是判断「曝光是不是推过头了」最快的手段；
 * - 鼠标在直方图上左右移动 → 高亮所在亮度段，便于读数。
 *
 * ## 为什么直方图按「编辑后」的值统计
 *
 * 直方图必须在**调整之后**统计，否则用户拉曝光时看到的分布不动，
 * 「把直方图推到中间」这个核心反馈环就断了。代价是每次改动都要重跑一遍
 * 渲染 —— 所以 store 里对它做了节流（180ms），且它与图像渲染是**两次独立
 * 的后端调用**（`edit_render` 与 `edit_histogram`），互不阻塞。
 */
const props = defineProps<{
  histogram: EditHistogram | null;
  /** 图宽（用于把截断比例换算成「占全图多少」时给用户一个直觉） */
  width: number;
  height: number;
}>();

const theme = useThemeStore();
const canvas = ref<HTMLCanvasElement | null>(null);
const hoverBin = ref(-1);

const W = 280;
const H = 96;
const PAD = 2;

const totalPixels = computed(() => Math.max(1, props.width * props.height));

/** 截断比例（0..1），用于决定是否点亮告警。 */
const lowRatio = computed(() => props.histogram?.clippedLow ?? 0);
const highRatio = computed(() => props.histogram?.clippedHigh ?? 0);

/** 告警阈值：0.5% 的像素被截断就值得提醒。 */
const CLIP_WARN = 0.005;

function draw(): void {
  const c = canvas.value;
  if (!c) return;
  const dpr = window.devicePixelRatio || 1;
  if (c.width !== W * dpr || c.height !== H * dpr) {
    c.width = W * dpr;
    c.height = H * dpr;
  }
  const g = c.getContext("2d");
  if (!g) return;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, W, H);

  const h = props.histogram;
  if (!h || h.luma.length !== 256) {
    g.fillStyle = "rgba(128,128,138,0.45)";
    g.font = '11px system-ui, -apple-system, "PingFang SC", sans-serif';
    g.textAlign = "center";
    g.fillText("直方图待载入", W / 2, H / 2);
    return;
  }

  // 峰值：取 RGB 与亮度的最大值，并保证至少为 1（避免全黑图除零）
  let peak = 1;
  for (let i = 0; i < 256; i++) {
    peak = Math.max(peak, h.luma[i]);
    for (let ch = 0; ch < 3; ch++) peak = Math.max(peak, h.rgb[i * 3 + ch] ?? 0);
  }

  const bw = (W - PAD * 2) / 256;
  const scaleY = (H - PAD * 2) / peak;

  const colors = ["rgba(235,72,78,0.5)", "rgba(96,190,96,0.5)", "rgba(86,140,236,0.55)"];

  // RGB 通道
  for (let ch = 0; ch < 3; ch++) {
    g.fillStyle = colors[ch];
    g.beginPath();
    g.moveTo(PAD, H - PAD);
    for (let i = 0; i < 256; i++) {
      const v = h.rgb[i * 3 + ch] ?? 0;
      const y = H - PAD - v * scaleY;
      g.lineTo(PAD + i * bw, y);
      g.lineTo(PAD + (i + 1) * bw, y);
    }
    g.lineTo(PAD + W - PAD * 2, H - PAD);
    g.closePath();
    g.fill();
  }

  // 亮度包络（白色描边）
  g.strokeStyle = hoverBin.value >= 0 ? "rgba(255,255,255,0.95)" : "rgba(240,244,252,0.7)";
  g.lineWidth = hoverBin.value >= 0 ? 1.6 : 1;
  g.beginPath();
  for (let i = 0; i < 256; i++) {
    const y = H - PAD - h.luma[i] * scaleY;
    if (i === 0) g.moveTo(PAD, y);
    else g.lineTo(PAD + i * bw, y);
  }
  g.stroke();

  // 悬停高亮条
  if (hoverBin.value >= 0) {
    g.fillStyle = "rgba(255,255,255,0.14)";
    g.fillRect(PAD + hoverBin.value * bw - 1, 0, bw + 2, H);
  }
}

function onMove(e: MouseEvent): void {
  const el = canvas.value;
  if (!el) return;
  const r = el.getBoundingClientRect();
  const x = (e.clientX - r.left) / r.width;
  hoverBin.value = Math.max(0, Math.min(255, Math.round(x * 255)));
}

function onLeave(): void {
  hoverBin.value = -1;
}

watch(() => props.histogram, draw, { deep: true });
watch(hoverBin, draw);
onMountedCompat();

function onMountedCompat(): void {
  draw();
}

onBeforeUnmount(() => {
  /* 无需清理（canvas 由 GC 回收） */
});

/** 悬停位置对应的像素占比读数。 */
const hoverReadout = computed(() => {
  const h = props.histogram;
  if (!h || hoverBin.value < 0) return "";
  const v = h.luma[hoverBin.value] ?? 0;
  const pct = (v / totalPixels.value) * 100;
  return `${hoverBin.value} · ${pct < 0.01 ? "<0.01" : pct.toFixed(2)}%`;
});

const clipStyle = computed(() => ({
  color: theme.subTextColor,
  background: "rgba(120,120,130,0.16)",
  border: `1px solid ${theme.cardBorder}`,
}));
</script>

<template>
  <div class="histo-panel">
    <div class="histo-head">
      <span class="histo-title">直方图</span>
      <span class="histo-readout">{{ hoverReadout }}</span>
    </div>
    <canvas
      ref="canvas"
      :width="W"
      :height="H"
      class="histo-canvas"
      @mousemove="onMove"
      @mouseleave="onLeave"
    ></canvas>
    <div class="histo-clip-row">
      <span
        class="histo-clip"
        :class="{ warn: lowRatio / totalPixels > CLIP_WARN }"
        :style="clipStyle"
        title="暗部被压到纯黑的像素数"
      >
        ◀ {{ lowRatio }}
      </span>
      <span
        class="histo-clip"
        :class="{ warn: highRatio / totalPixels > CLIP_WARN }"
        :style="clipStyle"
        title="高光被推到纯白的像素数"
      >
        {{ highRatio }} ▶
      </span>
    </div>
  </div>
</template>

<style scoped>
.histo-panel {
  width: 280px;
}
.histo-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  margin-bottom: 4px;
  font-size: 11px;
  color: var(--pm-label, var(--color-text-2));
}
.histo-readout {
  font-variant-numeric: tabular-nums;
  color: var(--pm-hint, var(--color-text-2));
}
.histo-canvas {
  display: block;
  width: 280px;
  height: 96px;
  border-radius: 6px;
  background: rgba(20, 22, 28, 0.28);
  border: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.22));
  cursor: crosshair;
}
.histo-clip-row {
  display: flex;
  justify-content: space-between;
  margin-top: 4px;
}
.histo-clip {
  padding: 1px 7px;
  font-size: 10px;
  border-radius: 8px;
  font-variant-numeric: tabular-nums;
}
.histo-clip.warn {
  color: #f0c36d;
  background: rgba(240, 195, 109, 0.18);
  border-color: rgba(240, 195, 109, 0.4);
}
</style>