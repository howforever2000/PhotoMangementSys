<script setup lang="ts">
/**
 * 支持栖影（打赏入口弹窗）【FEAT-199】
 *
 * 由 `views/Home.vue` 的 ⚙ 设置菜单打开，渲染时 teleport 到 body
 * （页头 `.user-box` 带 backdrop-filter，会把它变成 position:fixed 的包含块）。
 *
 * 设计取舍：
 *   · 二维码用**字节原样入库的素材**（`assets/support/wechat-qr.jpg`，解码实测
 *     为微信「关注公众号」跳转码，见 `SUPPORT_QR_DECODED`），由 `wechatQr.test.ts`
 *     钉住 SHA-256 —— 换图或误传别的码，单测直接红；
 *   · 码内容 ≠ 文章链接是事实而非失误（2026-11-25 cv2 解码核对），两条 URL
 *     在 `utils/support.ts` 里分开维护；
 *   · 文案不许写成「购买/解锁」：本项目是自愿赞助，功能全部免费
 *     （许可证 PolyForm Noncommercial，商用需另行授权）。
 */
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useThemeStore } from "../stores/theme";
import qrSrc from "../assets/support/wechat-qr.jpg";
import {
  SUPPORT_CREATOR,
  SUPPORT_PLATFORM,
  SUPPORT_QR_DECODED,
  SUPPORT_URL,
  openSupportPage,
} from "../utils/support";

const emit = defineEmits<{ close: [] }>();
const theme = useThemeStore();

const actionError = ref("");
const copyState = ref<"idle" | "ok" | "fail">("idle");
let copyTimer: ReturnType<typeof setTimeout> | null = null;

const copyLabel = computed(() => {
  if (copyState.value === "ok") return "已复制";
  if (copyState.value === "fail") return "复制失败";
  return "复制链接";
});

/* Esc 关闭由本组件自持（capture 监听 + 阻止冒泡）：
   Home 的 Esc 处理器负责它自己的另两个弹窗，这里不重复接管，也不让它被连带关闭。 */
function onKey(e: KeyboardEvent) {
  if (e.key !== "Escape") return;
  e.preventDefault();
  e.stopPropagation();
  emit("close");
}
onMounted(() => document.addEventListener("keydown", onKey, true));
onBeforeUnmount(() => {
  document.removeEventListener("keydown", onKey, true);
  if (copyTimer) clearTimeout(copyTimer);
});

async function onOpen() {
  actionError.value = "";
  try {
    await openSupportPage();
  } catch (e) {
    actionError.value = `打开浏览器失败：${String(e)}`;
  }
}

async function onCopy() {
  actionError.value = "";
  try {
    await navigator.clipboard.writeText(SUPPORT_URL);
    copyState.value = "ok";
  } catch {
    // 剪贴板不可用（权限/非安全上下文）时给出口：上面的地址本身可整体选中
    copyState.value = "fail";
  }
  if (copyTimer) clearTimeout(copyTimer);
  copyTimer = setTimeout(() => (copyState.value = "idle"), 1800);
}
</script>

<template>
  <teleport to="body">
    <div class="pm-modal" @click.self="emit('close')">
      <div
        class="pm-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="support-title"
        :style="theme.dialogVarStyle"
      >
        <div class="pm-dialog-head">
          <h3 id="support-title">支持栖影</h3>
          <span class="pm-hint">自愿赞助 · 不解锁任何功能</span>
        </div>

        <p class="sd-lede">
          栖影是一个人业余做的本地相册，全部功能免费，打赏与否都能照常用。
          如果你愿意支持，可以扫码关注{{ SUPPORT_PLATFORM }}——更新和打赏（文章内「喜欢作者」）都在那里，费用会用在代码签名证书、模型托管和测试设备上。
        </p>

        <div class="sd-qr">
          <img
            class="sd-qr-img"
            :src="qrSrc"
            :alt="`${SUPPORT_PLATFORM}二维码`"
            width="196"
            height="196"
            decoding="async"
          />
          <span class="sd-qr-cap">{{ SUPPORT_PLATFORM }} · {{ SUPPORT_CREATOR }}</span>
        </div>

        <p class="sd-verify">
          请用微信扫码；码内容已解码核对（{{ SUPPORT_QR_DECODED }}），是关注入口而非收款码。
        </p>

        <p class="sd-verify">最新文章也可直接在浏览器打开：</p>

        <p class="sd-url" :title="SUPPORT_URL">{{ SUPPORT_URL }}</p>

        <p v-if="actionError" class="pm-error">{{ actionError }}</p>

        <div class="pm-actions">
          <button class="pm-btn" type="button" @click="onCopy">{{ copyLabel }}</button>
          <button class="pm-btn pm-btn-primary" type="button" @click="onOpen">
            在浏览器打开
          </button>
        </div>

        <p class="sd-note">
          赞助不构成商业授权。本项目采用 PolyForm Noncommercial 1.0.0 许可，个人与非商业用途免费，商业使用需另行书面授权。
        </p>
      </div>
    </div>
  </teleport>
</template>

<style scoped>
.sd-lede {
  margin: 0 0 4px;
  font-size: 13px;
  line-height: 1.65;
  color: var(--pm-label);
}

/* 二维码区：素材居中；二维码本身黑白，刻意不吃主题令牌（半透明/深底近白会扫不出来） */
.sd-qr {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 16px 0 12px;
}

.sd-qr-img {
  display: block;
  border-radius: 8px;
  box-shadow: 0 0 0 1px var(--pm-soft-border);
  /* 素材是 430×430 的 QQ 原图（字节原样入库）：按 196 显示交给浏览器平滑
     缩小，边缘仍清晰；不要加 image-rendering: pixelated（会出锯齿）。 */
}

.sd-qr-cap {
  font-size: 12px;
  color: var(--pm-hint);
  letter-spacing: 0.02em;
}

.sd-verify {
  margin: 0 0 10px;
  font-size: 12.5px;
  line-height: 1.6;
  color: var(--pm-label);
}

.sd-verify b {
  color: var(--pm-text);
  font-weight: 600;
}

/* 地址整条可选中（复制失败时用户仍能手动取） */
.sd-url {
  margin: 0 0 12px;
  padding: 8px 10px;
  font-size: 12px;
  line-height: 1.5;
  word-break: break-all;
  user-select: all;
  color: var(--pm-label);
  background: var(--pm-input-bg);
  border: 1px solid var(--pm-soft-border);
  border-radius: 8px;
}

.sd-note {
  margin: 14px 0 0;
  font-size: 11.5px;
  line-height: 1.6;
  color: var(--pm-hint);
}
</style>
