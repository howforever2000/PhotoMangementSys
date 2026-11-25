/**
 * `utils/support.ts` 单元测试（FEAT-199，2026-11-25 迁移到微信公众号）
 *
 * 打赏入口的安全边界压在两处：
 *   · 「浏览器打开的那条 URL」是不是本项目的公众号文章 —— 这里锁死
 *     协议、域名、路径，改错一个字，用户就会被带去仿冒页面；
 *   · 「二维码图片素材」是不是原样入库的那张 —— 由 `wechatQr.test.ts`
 *     的 SHA-256 钉死，两边合起来覆盖「图与链接不一致」。
 */
import { describe, test } from "node:test";
import assert from "node:assert/strict";

import {
  SUPPORT_CREATOR,
  SUPPORT_HOST,
  SUPPORT_PLATFORM,
  SUPPORT_QR_DECODED,
  SUPPORT_URL,
  isOfficialSupportUrl,
} from "./support.ts";

describe("赞助入口地址", () => {
  test("必须是 https", () => {
    assert.equal(new URL(SUPPORT_URL).protocol, "https:");
  });

  test("域名是微信公众号平台", () => {
    assert.equal(SUPPORT_HOST, "mp.weixin.qq.com");
    assert.equal(
      SUPPORT_URL,
      "https://mp.weixin.qq.com/s/0lVyyRfoXZw4PcZH-lLYTw",
    );
  });

  test("平台名与公众号展示名来自单一事实源", () => {
    assert.equal(SUPPORT_PLATFORM, "微信公众号");
    assert.equal(SUPPORT_CREATOR, "howforever灵光匣");
  });

  test("二维码解码结果与文章链接是两条不同的地址（设计如此，防呆记录）", () => {
    assert.notEqual(SUPPORT_QR_DECODED, SUPPORT_URL);
    assert.equal(new URL(SUPPORT_QR_DECODED).host, "weixin.qq.com");
  });
});

describe("isOfficialSupportUrl", () => {
  test("官方地址通过", () => {
    assert.equal(isOfficialSupportUrl(SUPPORT_URL), true);
  });

  test("仿冒前缀域名被拒", () => {
    assert.equal(
      isOfficialSupportUrl("https://mp.weixin.qq.com.evil.example/s/0lVyyRfoXZw4PcZH-lLYTw"),
      false,
    );
  });

  test("仿冒后缀域名被拒", () => {
    assert.equal(
      isOfficialSupportUrl("https://notweixin.com/s/0lVyyRfoXZw4PcZH-lLYTw"),
      false,
    );
  });

  test("http 明文被拒", () => {
    assert.equal(
      isOfficialSupportUrl("http://mp.weixin.qq.com/s/0lVyyRfoXZw4PcZH-lLYTw"),
      false,
    );
  });

  test("换了路径（指向别人的文章）被拒", () => {
    assert.equal(
      isOfficialSupportUrl("https://mp.weixin.qq.com/s/someoneelse-article"),
      false,
    );
  });

  test("不是 URL 的字符串被拒", () => {
    assert.equal(isOfficialSupportUrl("mp.weixin.qq.com/s/0lVyyRfoXZw4PcZH-lLYTw"), false);
    assert.equal(isOfficialSupportUrl(""), false);
  });
});
