/**
 * 打赏二维码素材的完整性测试（FEAT-199，2026-11-25 迁移到微信公众号）
 *
 * 图片一旦被换成别人的码，用户扫码就会关注错人 —— 这类失误在 diff 里
 * 只表现为「二进制文件变了」，肉眼很难发现。所以这里把素材的 SHA-256 钉死：
 * 换图必须同步改本文件的常量，改常量必须先想清楚「这码指向谁」。
 *
 * 素材来源：QQ 聊天原图（430×430 JPEG）字节原样入库，未重编码。
 * 解码核对结果 `http://weixin.qq.com/r/mp/3hklPc3EfpABrYle90zM`
 * （微信「关注公众号」跳转码）记录在 `utils/support.ts` 的 `SUPPORT_QR_DECODED`，
 * 并由 `support.test.ts` 锁定 —— 图与链接的对应关系两边共同钉住。
 */
import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** 素材路径（相对本测试文件） */
const FILE = fileURLToPath(new URL("./wechat-qr.jpg", import.meta.url));

/**
 * 钉住的 SHA-256。
 * 内容：微信公众号关注码（见 SUPPORT_QR_DECODED），430×430 JPEG 原图字节。
 * 入库前已用 cv2.QRCodeDetector 回读，解码结果与 `SUPPORT_QR_DECODED` 完全一致。
 */
const PINNED_SHA256 = "f664b4c8e6eb6bc7a02134a704ba5b546bbef1cd3af326b17ee2bc48755dd70d";

/** JPEG 文件头 4 字节（SOI + APP0 标记） */
const JPEG_SIGNATURE = Buffer.from([0xff, 0xd8, 0xff, 0xe0]);

describe("打赏二维码素材", () => {
  const bytes = readFileSync(FILE);

  test("SHA-256 与钉住的值一致（换图即红）", () => {
    assert.equal(createHash("sha256").update(bytes).digest("hex"), PINNED_SHA256);
  });

  test("确实是 JPEG（不是被换成别的格式改了后缀）", () => {
    assert.equal(bytes.subarray(0, 4).equals(JPEG_SIGNATURE), true);
  });
});
