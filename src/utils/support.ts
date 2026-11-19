/**
 * 打赏入口的**唯一事实源**（FEAT-199）
 *
 * ## 为什么不用微信 / 支付宝个人收款码
 *
 * 个人静态收款码有四个绕不开的问题，且都不是「把图片加密」能解决的：
 *   ① **不可吊销** —— 静态码没有作废动作，换码之后旧码继续能收钱。一旦泄露或被
 *      贴到别处，作者无法关闭它。
 *   ② **远程收款合规** —— 个人静态收款码原则上不得用于远程非面对面收款
 *      （银发〔2021〕259 号口径），把码公开放在软件里让人远程扫，存在账户被风控的风险。
 *   ③ **暴露收款账户** —— 收款码内含可被反查的账户标识，公开传播等于把收款入口交出去。
 *   ④ **无法对账** —— 没有订单号，不知道谁付的、也无法按单退款。
 *
 * 改用平台代收（爱发电）后：收款主体是平台，个人账户不暴露；页面可随时改绑/下线；
 * 有订单与留言可对账。
 *
 * ## 二维码为什么是一张「被钉住的素材」而不是随手截的图
 *
 * `assets/support/afdian-qr.png` 由 `SUPPORT_URL` 生成（592×592、每模块 16px、quiet zone 4），
 * 生成后用独立解码器回读校验「解出来 === SUPPORT_URL」，并把 SHA-256 钉进
 * `assets/support/afdianQr.test.ts`：图片被换掉或被误传成别的码，单测立刻红。
 *
 * 反面教材（2026-11-19 实测）：爱发电「分享图」里那张二维码解出来是
 * `afdian.com/dashboard/stats?utm_source=qrpic&utm_medium=link` —— 创作者后台 + 追踪参数，
 * 不是公开赞助页。**任何二维码进仓前都必须先解码核对目标地址**。
 */

import { openExternalUrl } from "./openExternal.ts";


/** 赞助页地址（全仓唯一出现处；改这里即改全站） */
export const SUPPORT_URL = "https://afdian.com/a/howforever";

/** 收款平台名（弹窗里给用户核对用） */
export const SUPPORT_PLATFORM = "爱发电";

/** 平台域名：从 URL 派生，禁止再写死第二份 */
export const SUPPORT_HOST = new URL(SUPPORT_URL).host;

/** 创作者主页标识（取自 URL slug；弹窗里不直接展示，展示用 `SUPPORT_CREATOR`） */
export const SUPPORT_AUTHOR = new URL(SUPPORT_URL).pathname.replace(/^\/a\//, "");

/**
 * 爱发电主页的**展示名**（2026-11-19 实拍主页确认：创作者名为 `hao`，
 * 与 URL slug `howforever` 不一致 —— 两者别混用，用户核对时看的是展示名）。
 */
export const SUPPORT_CREATOR = "hao";

/**
 * 自检：要打开的地址确实是本项目的赞助页。
 *
 * 判据刻意收得很紧（https + 域名全等 + 路径全等）——「长得像」的仿冒域名
 * （`afdian.com.evil.example`、`afdian.hk`）一律拒掉。这是本方案唯一可篡改的
 * 数据，值得多一道运行时检查。
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

/** 打开赞助页（调用前先过 `isOfficialSupportUrl`，防止被改成钓鱼地址） */
export async function openSupportPage(): Promise<void> {
  if (!isOfficialSupportUrl(SUPPORT_URL)) {
    throw new Error("赞助页地址校验未通过，已阻止打开");
  }
  await openExternalUrl(SUPPORT_URL);
}
