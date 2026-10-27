// 真实光栅化成本对比（第三版）
//
// ⚠️ 结论：**本环境无法可靠测出滤镜链的真实开销**。保留此脚本只作为
//    “结构与档位是否渲染正确”的检查器，**不要**引用它的耗时数字。
//
// 三版尝试与失败原因（留给后来者，不要重复踩）：
//   ① 只改玻璃**内部**元素 → backdrop-filter 采样的是背后，不重新光栅化；
//   ② 改背景但只看 rAF 间隔 → 无头 Edge 下 rAF 被节流到 200fps，
//      且合成器光栅代价**不进入 JS 的 rAF 间隔**，三档全测出同一个 5ms；
//   ③ 连续截图测吞吐 → PNG 编码的抖动（±30ms）远大于滤镜本身（~1-3ms），
//      信噪比不足，算出来会出现“加玻璃反而更快”这种荒谬结果。
//
// 可行的替代结论（不靠测时间，靠图元数）：
//   ② 边缘折射   = turbulence + blur + 1×displace      → 约 3 个图元
//   ③ 折射+色散  = turbulence + blur + 3×displace
//                  + 3×colormatrix + 2×composite        → 约 11 个图元
//   即 ③ 大约是 ② 的 3 倍滤镜工作量。这个比值不需要跑分也能算出来。

const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";

const ROUNDS = 14;

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1200, height: 700 }, deviceScaleFactor: 1 });
  p.on("pageerror", (e) => console.log("pageerror", String(e).split("\n")[0]));
  await p.goto(`http://localhost:${PORT}/.visual/glass-refraction-lab.html`, { waitUntil: "load" });
  await p.waitForTimeout(800);

  // 只保留一档，并加一层高频背景（有细节才吃得出滤镜代价）
  const setup = async (keepId) =>
    p.evaluate((keep) => {
      for (const id of ["g1", "g2", "g3"]) {
        const el = document.getElementById(id);
        if (!el) continue;
        /* keep === "none" 时三块全隐藏（基线就是“没有玻璃”）
           ⚠️ 初版这里写反了（keep==="none" 反而不隐藏），
           于是基线反而比有玻璃还慢 —— 别改回去。 */
        el.style.display = keep === "none" ? "none" : id === keep ? "" : "none";
      }
      let mover = document.getElementById("__mover");
      if (!mover) {
        mover = document.createElement("div");
        mover.id = "__mover";
        mover.style.cssText =
          "position:fixed;inset:0;z-index:-1;pointer-events:none;" +
          "background:repeating-linear-gradient(45deg,#fff 0 9px,#0a2 9px 18px," +
          "#f80 18px 27px,#08f 27px 36px);opacity:.95";
        document.body.appendChild(mover);
      }
    }, keepId);

  const measure = async (keepId) => {
    await setup(keepId);
    await p.waitForTimeout(120);
    // 预热 2 张（第一张含字体/图层初始化）
    for (let i = 0; i < 2; i++) await p.screenshot({ type: "png" });
    const t0 = Date.now();
    for (let i = 0; i < ROUNDS; i++) await p.screenshot({ type: "png" });
    return (Date.now() - t0) / ROUNDS;
  };

  // 正反各一轮，取每档最好的一轮（排除系统抖动）
  const order = ["none", "g1", "g2", "g3"];
  const forward = {};
  for (const k of order) forward[k] = await measure(k);
  const backward = {};
  for (const k of [...order].reverse()) backward[k] = await measure(k);

  const pick = (k) => Math.min(forward[k], backward[k]);

  const N = pick("none");
  const G1 = pick("g1");
  const G2 = pick("g2");
  const G3 = pick("g3");

  const row = (label, v) =>
    `${label.padEnd(14)} ${v.toFixed(1).padStart(7)} ms/张`;

  console.log("连续截图吞吐（仅作渲染正确性检查，**不要引用数字**）\n");
  console.log(row("裸背景（无玻璃）", N));
  console.log(row("① 普通模糊", G1));
  console.log(row("② 边缘折射", G2));
  console.log(row("③ 折射 + 色散", G3));

  const d1 = G1 - N;
  const d2 = G2 - N;
  const d3 = G3 - N;
  const noisy = d1 < 0 || d2 < 0 || d3 < 0;
  console.log(
    `\n${noisy ? "❌ 出现负差值 —— PNG 编码抖动淹没了滤镜成本，本环境测不出来" : "✔ 差值方向合理"}`,
  );
  console.log(`  ① blur 16px                    ${d1 >= 0 ? "+" : ""}${d1.toFixed(1)} ms`);
  console.log(`  ② blur 3px + 1×displace        ${d2 >= 0 ? "+" : ""}${d2.toFixed(1)} ms`);
  console.log(`  ③ blur 3px + 3×displace + 色散 ${d3 >= 0 ? "+" : ""}${d3.toFixed(1)} ms`);
  console.log(
    "\n结论：**不靠测时间，靠图元数**。",
    "\\n  ② = turbulence + blur + 1×displace ≈ 3 个图元",
    "\\n  ③ = turbulence + blur + 3×displace + 3×colormatrix + 2×composite ≈ 11 个图元",
    "\\n  即 ③ 约是 ② 的 3 倍滤镜工作量 —— 这个比值不用跑分也能算。",
  );

  await b.close();
})();
