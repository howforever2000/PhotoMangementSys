/**
 * `utils/loginTheme.ts` 单元测试 —— FEAT-093 登录/注册/忘记密码三页共用配色
 *
 * 钉死四条不变量（全部按**最坏情况**：卡片压在壁纸最亮的粉带上）：
 *   1. 卡片玻璃上的标题/副标题/标签/链接 ≥ 4.5:1；
 *   2. 输入框内的正文与占位符 ≥ 4.5:1，框线 vs 框底 ≥ 3:1（WCAG 1.4.11 控件边界）；
 *   3. 主按钮（金底墨字）≥ 4.5:1，且金底上白字必然不达标——把「不许改回白字」钉住；
 *   4. 遮罩有效：最暗壁纸端对比高于最亮端（单调），且非法输入不抛错、不走坏值。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { contrastRatio, hexToRgb, mixRgb, relLum, rgbToHex, type Rgb } from "./color.ts";
import {
  AUTH_PALETTE,
  AUTH_SCRIM,
  AUTH_WALLPAPER_SAMPLES,
  authCssVars,
  authFieldBg,
  authGlass,
  authMsgBg,
  authScrimBackground,
} from "./loginTheme.ts";
import { DEFAULTS } from "./prefs.ts";

test("登录页与应用默认外观同源：组件色调与壁纸必须一致（同一张启动封面）", () => {
  // 视觉上「登录页 = 应用默认外观」是本次改动的核心承诺；色调/壁纸一漂移就会两套世界
  assert.equal(AUTH_PALETTE.cardTone, DEFAULTS.compColor, "登录卡色调应与默认组件色调同色");
  assert.equal(DEFAULTS.bgStyle, "image", "应用默认必须是背景图模式（即启动封面）");
});

const glassBright = authGlass(AUTH_WALLPAPER_SAMPLES.brightest);
const glassDark = authGlass(AUTH_WALLPAPER_SAMPLES.darkest);

function ratio(fg: string, bg: Rgb): number {
  return contrastRatio(relLum(hexToRgb(fg)), relLum(bg));
}

test("卡片玻璃（最亮壁纸带）上的文字对比 ≥ 4.5:1", () => {
  const roles: [string, string][] = [
    ["标题", AUTH_PALETTE.title],
    ["副标题", AUTH_PALETTE.sub],
    ["字段标签", AUTH_PALETTE.label],
    ["链接", AUTH_PALETTE.link],
  ];
  for (const [name, color] of roles) {
    const r = ratio(color, glassBright);
    assert.ok(r >= 4.5, `${name} ${color} 在最亮壁纸带上仅 ${r.toFixed(2)}:1 < 4.5`);
  }
});

test("最暗壁纸端对比更高（遮罩/玻璃单调，暗处不会反而更难读）", () => {
  for (const color of [AUTH_PALETTE.title, AUTH_PALETTE.sub, AUTH_PALETTE.link]) {
    assert.ok(
      ratio(color, glassDark) > ratio(color, glassBright),
      `${color} 暗端应比亮端更清晰`,
    );
  }
});

test("输入框：正文/占位符 ≥ 4.5:1，框线 vs 框底 ≥ 3:1", () => {
  const field = authFieldBg(glassBright);
  for (const [name, color] of [
    ["输入文字", AUTH_PALETTE.title],
    ["占位符", AUTH_PALETTE.placeholder],
  ] as [string, string][]) {
    const r = ratio(color, field);
    assert.ok(r >= 4.5, `${name} ${color} 在输入框底上仅 ${r.toFixed(2)}:1 < 4.5`);
  }
  const borderRgb = mixRgb(field, hexToRgb(AUTH_PALETTE.fieldBorder), AUTH_PALETTE.fieldBorderAlpha);
  const borderRatio = contrastRatio(relLum(borderRgb), relLum(field));
  assert.ok(borderRatio >= 3, `输入框框线 ${rgbToHex(borderRgb)} 对比仅 ${borderRatio.toFixed(2)}:1 < 3`);
});

test("主按钮：金底墨字 ≥ 4.5:1；金底白字必然不达标（不许改回白字）", () => {
  for (const [name, bg] of [
    ["常态", AUTH_PALETTE.accent],
    ["悬停", AUTH_PALETTE.accentHover],
  ] as [string, string][]) {
    const inkRatio = ratio(AUTH_PALETTE.accentInk, hexToRgb(bg));
    assert.ok(inkRatio >= 4.5, `${name} 按钮墨字对比 ${inkRatio.toFixed(2)}:1 < 4.5`);
    const whiteRatio = ratio("#ffffff", hexToRgb(bg));
    assert.ok(
      whiteRatio < 4.5,
      `${name} 按钮若改白字对比 ${whiteRatio.toFixed(2)}:1 —— 该断言用于固定「金底必须配墨字」这一设计约束`,
    );
  }
});

test("提示条：危险/成功文字在自身底色上 ≥ 4.5:1", () => {
  const dangerBg = authMsgBg(glassBright, AUTH_PALETTE.dangerBg);
  const okBg = authMsgBg(glassBright, AUTH_PALETTE.okBg);
  const dr = contrastRatio(relLum(hexToRgb(AUTH_PALETTE.dangerText)), relLum(dangerBg));
  const okr = contrastRatio(relLum(hexToRgb(AUTH_PALETTE.okText)), relLum(okBg));
  assert.ok(dr >= 4.5, `危险提示对比 ${dr.toFixed(2)}:1 < 4.5`);
  assert.ok(okr >= 4.5, `成功提示对比 ${okr.toFixed(2)}:1 < 4.5`);
});

test("CSS 变量包：令牌齐全且与常量同源，遮罩层数正确", () => {
  const vars = authCssVars();
  for (const key of [
    "--auth-card-bg",
    "--auth-card-border",
    "--auth-title",
    "--auth-sub",
    "--auth-label",
    "--auth-link",
    "--auth-placeholder",
    "--auth-field-bg",
    "--auth-field-border",
    "--auth-accent",
    "--auth-accent-hover",
    "--auth-accent-ink",
    "--auth-danger-text",
    "--auth-danger-bg",
    "--auth-ok-text",
    "--auth-ok-bg",
    "--auth-scrim",
  ]) {
    assert.ok(vars[key], `缺少令牌 ${key}`);
  }
  assert.equal(vars["--auth-title"], AUTH_PALETTE.title);
  assert.equal(vars["--auth-accent"], AUTH_PALETTE.accent);
  assert.equal(
    vars["--auth-field-bg"],
    `rgba(43,33,64,${AUTH_PALETTE.fieldAlpha})`,
    "输入框底应由组件色调 × fieldAlpha 推出",
  );
  assert.ok(vars["--auth-scrim"].includes("radial-gradient"), "遮罩应含暗角层");
  assert.ok(vars["--auth-scrim"].includes("linear-gradient"), "遮罩应含竖向压暗层");
  assert.equal(
    vars["--auth-scrim"].split("rgba").length - 1,
    4,
    "遮罩应恰好 4 个色标（暗角 2 + 竖向 2）",
  );
});

test("遮罩下限自洽：alphaUnderCard 不高于竖向渐变在该处的合成值", () => {
  // 卡片上沿约在视口 19% 处：竖向渐变 = top + 0.19×(bottom-top)，暗角只增不减
  const atCardTop = AUTH_SCRIM.top + 0.19 * (AUTH_SCRIM.bottom - AUTH_SCRIM.top);
  assert.ok(
    atCardTop >= AUTH_SCRIM.alphaUnderCard,
    `卡片上沿竖向遮罩 ${atCardTop.toFixed(3)} < 断言的 ${AUTH_SCRIM.alphaUnderCard}`,
  );
  assert.ok(AUTH_SCRIM.vignette >= 0 && AUTH_SCRIM.top < AUTH_SCRIM.bottom, "上浅下深");
});

test("非法输入不抛错：authGlass / authScrimBackground 走安全回落", () => {
  const g = authGlass("not-a-color");
  assert.equal(g.length, 3);
  for (const v of g) assert.ok(Number.isFinite(v) && v >= 0 && v <= 255, `分量越界: ${v}`);
  const scrim = authScrimBackground({ ...AUTH_SCRIM, color: "坏值" });
  assert.ok(scrim.startsWith("radial-gradient"), "坏色值应回落而不是产出 undefined");
  assert.ok(!scrim.includes("undefined") && !scrim.includes("NaN"));
});
