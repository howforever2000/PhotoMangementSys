/**
 * 打赏二维码素材的完整性测试（FEAT-199）
 *
 * 图片一旦被换成别人的收款码，用户扫码就会把钱付错人 —— 这类失误在 diff 里
 * 只表现为「二进制文件变了」，肉眼很难发现。所以这里把素材的 SHA-256 钉死：
 * 换图必须同步改本文件的常量，改常量必须先想清楚「这码指向谁」。
 *
 * 为什么是 SHA-256 而不是解码校验：解码器（jsQR）不进生产依赖，
 * 而目标地址已在 `utils/support.ts` 被单测锁死，两者合起来覆盖了「图与链接不一致」。
 */
import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** 素材路径（相对本测试文件） */
const FILE = fileURLToPath(new URL("./afdian-qr.png", import.meta.url));

/**
 * 钉住的 SHA-256。
 * 生成方式：内容 `https://afdian.com/a/howforever`、纠错 M、每模块 16px、quiet zone 4。
 * 生成后已用 jsQR 回读，解码结果与 `SUPPORT_URL` 完全一致。
 */
const PINNED_SHA256 = "5361132c7a6675d73d76dfdf78d2f1b96e0ddcb23a613531e00264a0fb5bb3bc";

/** PNG 文件头 8 字节 */
const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

describe("打赏二维码素材", () => {
  const bytes = readFileSync(FILE);

  test("SHA-256 与钉住的值一致（换图即红）", () => {
    assert.equal(createHash("sha256").update(bytes).digest("hex"), PINNED_SHA256);
  });

  test("确实是 PNG（不是被换成 svg/jpg 改了后缀）", () => {
    assert.equal(bytes.subarray(0, 8).equals(PNG_SIGNATURE), true);
  });

  test("尺寸为 592×592（29 模块 + 两侧 4 模块 quiet zone，×16px）", () => {
    // IHDR：宽度在偏移 16、高度在偏移 20，均为大端 uint32
    assert.equal(bytes.readUInt32BE(16), 592);
    assert.equal(bytes.readUInt32BE(20), 592);
  });
});
