/**
 * 内容分类卡片探针（FEAT-113 ③-2）
 *
 * 只渲染 CategoryGallery 的**卡片网格部分**是不现实的（它是一整个组件，
 * 内部自己 invoke 后端）。所以这里用同一套 class 名 + 同一条 CSS 规则，
 * 复制出网格结构：验证的是「排布公式」而不是组件本身。
 *
 * ⚠️ 关键：样式必须从 CategoryGallery.vue 里**抄一份**，不能自己重写 ——
 *    否则验证的是探针自己的 CSS，不是产品的。改动产品样式时要同步这里。
 */
import { createApp, h } from "vue";

const COLORS = [
  "#e8d9a8", "#a8c98b", "#9ec4e8", "#d6a8c8", "#f0b48a",
  "#b8a8e8", "#8ad8c8", "#e8b8b8", "#c8d8a8", "#d8b8f0",
  "#a8d8e8", "#f0c8a8", "#b0c8f0", "#e8c8d8",
];
const ICONS = ["🌃", "🥾", "🎉", "🌇", "🍜", "🌸", "🌲", "🤖", "📄", "🌊",
  "🏙️", "🍂", "🐱", "🪴"];
const NAMES = ["夜空", "森林徒步", "聚会合影", "日落晚霞", "美食", "花",
  "自然风景", "人像", "扫描", "海边", "城市夜景", "秋天", "猫咪", "绿植"];
const COUNTS = [31, 42, 24, 25, 57, 0, 36, 24, 18, 51, 22, 16, 29, 12];

const cards = NAMES.map((name, i) => ({
  id: i + 1,
  name,
  icon: ICONS[i],
  count: COUNTS[i],
  color: COLORS[i],
  keywords: i % 3 === 0 ? ["夜", "暗", "灯光", "城市"] : ["词1", "词2"],
  source: i % 4 === 0 ? "builtin" : "user",
}));

const CAT_H = [176, 208, 240, 192, 224, 256, 200, 232];
function catH(id: number) {
  const h = (id * 2654435761) >>> 0;
  return CAT_H[h % CAT_H.length];
}
function isWideCat(i: number) {
  return i % 5 === 4;
}

/* 与 CategoryGallery.vue 的 toneBg / toneFg 保持一致的近似：
   类目无色封面时用调色盘纯色，字色按底色明暗取深/浅。 */
function toneBg(c: string) {
  return `linear-gradient(135deg, ${c}, ${c})`;
}
function toneFg(c: string) {
  const r = parseInt(c.slice(1, 3), 16);
  const g = parseInt(c.slice(3, 5), 16);
  const b = parseInt(c.slice(5, 7), 16);
  const l = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
  return l > 0.6 ? "#2a2f3a" : "#f5f7fa";
}

const style = document.createElement("style");
style.textContent = `
:root { --dur-2: 200ms; }
body { background: #1b1530; color: #f0eef3; font-family: "Segoe UI", "Microsoft YaHei", sans-serif; }
.page { padding: 24px 52px 60px; }

/* ===== 以下规则抄自 CategoryGallery.vue，改产品时同步 ===== */
.cat-grid {
  display: grid; grid-template-columns: repeat(4, 1fr); grid-auto-rows: 186px;
  grid-auto-flow: dense; gap: 14px;
}
@media (max-width: 1280px) { .cat-grid { grid-template-columns: repeat(3, 1fr); } }
@media (max-width: 900px)  { .cat-grid { grid-template-columns: repeat(2, 1fr); } }

.cat-card {
  display: block; margin: 0; border-radius: 14px; overflow: hidden; cursor: pointer;
  box-shadow: 0 4px 14px rgba(0,0,0,.1);
}
.cat-card.is-wide { grid-column: span 2; }
.cat-cover {
  position: relative; width: 100%; height: 100%; overflow: hidden;
  display: flex; align-items: center; justify-content: center;
}

.cat-cover-ph { font-size: 40px; font-weight: 700; opacity: .75; }
.cat-count {
  position: absolute; right: 8px; top: 8px; z-index: 2; padding: 2px 9px;
  font-size: 11.5px; color: #fff; background: rgba(0,0,0,.55);
  border-radius: 999px; backdrop-filter: blur(3px);
}
.cat-seal {
  position: absolute; left: 0; right: 0; bottom: 0; z-index: 2;
  padding: 26px 12px 10px; display: flex; flex-direction: column;
  align-items: flex-start; gap: 1px; pointer-events: none;
}
.cat-name {
  margin: 0; max-width: 100%; font-size: 15px; font-weight: 700;
  color: var(--tone-fg, #475569); white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}
.cat-sub {
  max-width: 100%; font-size: 11px; color: var(--tone-fg, #475569);
  opacity: .72; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}
`;
document.head.appendChild(style);

createApp({
  setup() {
    return () =>
      h("div", { class: "page" }, [
        h(
          "div",
          { class: "cat-grid" },
          cards.map((t, ti) =>
            h(
              "article",
              { key: t.id, class: ["cat-card", { "is-wide": isWideCat(ti) }] },
              [
                h(
                  "div",
                  { class: "cat-cover", style: { background: toneBg(t.color), "--tone-fg": toneFg(t.color) } },
                  [
                    h("span", { class: "cat-cover-ph" }, t.icon || t.name.slice(0, 1)),
                    h("span", { class: "cat-count" }, `${t.count} 张`),
                    h("div", { class: "cat-seal" }, [
                      h("h3", { class: "cat-name" }, `${t.icon} ${t.name}`),
                      h("span", { class: "cat-sub" }, `${t.keywords.length} 词 · 匹配度 64`),
                    ]),
                  ],
                ),
              ],
            ),
          ),
        ),
      ]);
  },
}).mount("#app");
