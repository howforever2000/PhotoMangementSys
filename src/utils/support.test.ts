/**
 * `utils/support.ts` 单元测试（FEAT-199）
 *
 * 打赏入口的安全边界全部压在「那条 URL 是不是官方地址」上：
 * 二维码是运行时按它画出来的，图片本身不存在。所以这里把协议、域名、路径
 * 三条都锁死 —— 改错一个字，扫码的人就会付给仿冒站点。
 */
import { describe, test } from "node:test";
import assert from "node:assert/strict";

import {
  SUPPORT_AUTHOR,
  SUPPORT_CREATOR,
  SUPPORT_HOST,
  SUPPORT_PLATFORM,
  SUPPORT_URL,
  isOfficialSupportUrl,
} from "./support.ts";

describe("赞助页地址", () => {
  test("必须是 https", () => {
    assert.equal(new URL(SUPPORT_URL).protocol, "https:");
  });

  test("域名是爱发电主站", () => {
    assert.equal(SUPPORT_HOST, "afdian.com");
    assert.equal(SUPPORT_URL, "https://afdian.com/a/howforever");
  });

  test("平台名与主页标识来自单一事实源", () => {
    assert.equal(SUPPORT_PLATFORM, "爱发电");
    assert.equal(SUPPORT_AUTHOR, "howforever");
  });

  test("展示名与 URL slug 分开维护（主页实拍为 hao）", () => {
    assert.equal(SUPPORT_CREATOR, "hao");
    assert.notEqual(SUPPORT_CREATOR, SUPPORT_AUTHOR);
  });
});

describe("isOfficialSupportUrl", () => {
  test("官方地址通过", () => {
    assert.equal(isOfficialSupportUrl(SUPPORT_URL), true);
  });

  test("仿冒前缀域名被拒", () => {
    assert.equal(isOfficialSupportUrl("https://afdian.com.evil.example/a/howforever"), false);
  });

  test("仿冒后缀域名被拒", () => {
    assert.equal(isOfficialSupportUrl("https://notafdian.com/a/howforever"), false);
  });

  test("http 明文被拒", () => {
    assert.equal(isOfficialSupportUrl("http://afdian.com/a/howforever"), false);
  });

  test("换了路径（指向别人的主页）被拒", () => {
    assert.equal(isOfficialSupportUrl("https://afdian.com/a/someoneelse"), false);
  });

  test("不是 URL 的字符串被拒", () => {
    assert.equal(isOfficialSupportUrl("afdian.com/a/howforever"), false);
    assert.equal(isOfficialSupportUrl(""), false);
  });
});
