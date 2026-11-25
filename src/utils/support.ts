/**
 * 打赏入口的**唯一事实源**（FEAT-199，2026-11-25 迁移到微信公众号）
 *
 * ## 入口形态：公众号关注码 + 最新文章链接
 *
 * 打赏入口由爱发电迁移为微信公众号：用户扫码关注公众号，打赏在公众号
 * 文章内的「喜欢作者」完成；「在浏览器打开 / 复制链接」走最新一期文章
 * （`SUPPORT_URL`）。原爱发电入口（afdian.com/a/howforever）下线。
 *
 * ## 两条 URL 刻意分开维护
 *
 *   · `SUPPORT_QR_DECODED` —— 二维码**图片素材**实际编码的内容。用 cv2
 *     QRCodeDetector 解码 `assets/support/wechat-qr.jpg` 实测（2026-11-25）：
 *     `http://weixin.qq.com/r/mp/3hklPc3EfpABrYle90zM`，即微信「关注公众号」
 *     跳转码，**不是**文章链接、也不是收款码。
 *   · `SUPPORT_URL` —— 弹窗里展示 / 复制 / 浏览器打开的文章地址。
 *
 * 两者不一致是事实而非失误（反面教材的延续，2026-11-19 爱发电分享图实测：
 * 码内容 ≠ 页面地址）。任何二维码进仓前都必须先解码核对目标地址，
 * 解码结果就记录在这里，并被 `wechatQr.test.ts` 的 SHA-256 钉死对应关系。
 *
 * ## 为什么图片素材是「被钉住的文件」而不是随手截的图
 *
 * 素材来自 QQ 原图（430×430 JPEG），字节原样入库，SHA-256 钉进
 * `assets/support/wechatQr.test.ts`：图片被换掉或被误传成别的码，单测立刻红。
 *
 * 反面教材（2026-11-19 实测）：爱发电「分享图」里那张二维码解出来是
 * `afdian.com/dashboard/stats?utm_source=qrpic&utm_medium=link` —— 创作者后台 + 追踪参数，
 * 不是公开赞助页。**任何二维码进仓前都必须先解码核对目标地址**。
 */

import { openExternalUrl } from "./openExternal.ts";


/** 最新公众号文章地址（弹窗展示 / 复制链接 / 浏览器打开；全仓唯一出现处） */
export const SUPPORT_URL =
  "https://mp.weixin.qq.com/s/0lVyyRfoXZw4PcZH-lLYTw";

/**
 * 二维码素材（`assets/support/wechat-qr.jpg`）解码核对结果（2026-11-25 cv2 实测）。
 * 微信「关注公众号」跳转码：微信扫码 → 公众号资料页；浏览器打开无效属正常。
 */
export const SUPPORT_QR_DECODED = "http://weixin.qq.com/r/mp/3hklPc3EfpABrYle90zM";

/** 收款/关注平台名（弹窗里给用户核对用） */
export const SUPPORT_PLATFORM = "微信公众号";

/** 平台域名：从 URL 派生，禁止再写死第二份 */
export const SUPPORT_HOST = new URL(SUPPORT_URL).host;

/** 公众号展示名（2026-11-25 实读文章页署名：来自小程序 howforever灵光匣） */
export const SUPPORT_CREATOR = "howforever灵光匣";

/**
 * 自检：要打开的地址确实是本项目的公众号文章。
 *
 * 判据刻意收得很紧（https + 域名全等 + 路径全等）——「长得像」的仿冒域名
 * （`mp.weixin.qq.com.evil.example`）与**别人家的文章**一律拒掉。这是本方案
 * 唯一可篡改的数据，值得多一道运行时检查。
 */
export function isOfficialSupportUrl(url: string): boolean {
  try {
    const target = new URL(url);
    const official = new URL(SUPPORT_URL);
    return (
      target.protocol === "https:" &&
      target.host === official.host &&
      target.pathname === official.pathname
    );
  } catch {
    return false;
  }
}

/** 打开赞助入口（调用前先过 `isOfficialSupportUrl`，防止被改成钓鱼地址） */
export async function openSupportPage(): Promise<void> {
  if (!isOfficialSupportUrl(SUPPORT_URL)) {
    throw new Error("赞助页地址校验未通过，已阻止打开");
  }
  await openExternalUrl(SUPPORT_URL);
}
