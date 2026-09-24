"""P-OPT2 实测：/classify_batch 照片级并行能提多少吞吐？哪个并行度最优？

背景与动机
----------
真机日志（2026-09-22 19:18，album 32 · 824 张）显示 classify 单张 ~474.8ms，
与 P-OPT1 之前的 ~461ms/张 基本持平 —— 通道级两流并行的收益被 tone 占比（~9%）
锁死，属于「正确但封顶低」。而第二轮 CPU 截图实测：classify 期间 16 核总利用率
仅 ~12%（只有 1~2 核在动）。det/face/ocr 会话 intra_op = threads()（默认物理核
夹 [4,8] ⇒ 本机 8），模型小、算子短，intra-op 吃不满机器；单张里还有一段串行
PIL 解码（4096px 原图，数十 ms）夹在两次推理之间。

⇒ 下一个并行维度是「照片」：一个线程处理一张照片，P 张同时在飞——
  - 下一张的解码与当前张的 ONNX 推理重叠（串行段的确定性收益）
  - 填补 ONNX intra-op 的同步缝隙
本脚本回答：同一 CPU 预算下 P=1/2/3/4 各能到多少 ms/张？哪个最优？
以及输出等价性（除 person_ids 允许置换外逐字段一致）。

口径与纪律
----------
- 与生产 /classify_batch 完全同构：路径按 BATCH_CHUNK(8) 张分块 → 每块用
  ThreadPoolExecutor(P) 对 classify_one 做 map（**解码计入测量**，与生产一致；
  P-OPT1 的 bench 解码后复用，那是通道归因口径，不是吞吐口径）。
- 照片取自真实相册目录（用户刚扫过的册子），不造数据。
- 预热 1 轮 + 正式 N 轮取最快，交错轮转（基准四条硬规矩）。
- 人物库隔离（safe_bench）：绝不碰生产 persons.db。每次计时/等价跑之前 reset_store，
  保证各档从同一空库状态出发（face 的 register 开销各档一致，不污染对比）。
- 等价性判据：category / sub_category / label / confidence / person_count 逐字段
  一致；**person_ids 允许置换**——register() 已用 _FACE_STORE_LOCK 串行化（同人
  分组结构不变），但完成顺序不定会让「新人编号」的分配顺序不同。这是照片级
  并行的固有语义，不是缺陷；distinct person 总数应一致。

用法
----
    python python/bench/photo_parallel_bench.py "<照片目录>" [--limit 32]
        [--rounds 3] [--levels 1,2,3,4] [--out 报告.txt]
"""
import argparse
import concurrent.futures
import os
import sqlite3
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
sys.path.insert(0, ROOT)

# 必须最先导入：把人物库隔离到临时目录，绝不碰生产 persons.db
# （2026-09-22 事故：bench 脚本曾清空生产库，详见 safe_bench.py 顶部说明）
import safe_bench  # noqa: E402,F401

from vcr import config  # noqa: E402
from vcr.model_registry import get_registry  # noqa: E402
from vcr.services import pipeline  # noqa: E402

safe_bench.assert_isolated()   # 导入 vcr 之后立刻确认隔离生效

EXTS = (".jpg", ".jpeg", ".png", ".webp", ".bmp")
CHUNK = config.BATCH_CHUNK  # 与生产 /classify_batch 的客户端批次一致（8）


def list_photos(root: str, limit: int = 0) -> list[str]:
    out: list[str] = []
    for dp, _dn, fn in os.walk(root):
        for f in fn:
            if f.lower().endswith(EXTS):
                out.append(os.path.join(dp, f))
    out.sort()
    return out[:limit] if limit else out


def reset_store() -> None:
    """清空【隔离环境】的人物库，让各档从同一空库状态出发（生产库在本进程不可见）。"""
    con = sqlite3.connect(config.PERSONS_DB)
    try:
        for tbl in ("faces", "persons"):
            try:
                con.execute(f"DELETE FROM {tbl}")
            except sqlite3.OperationalError:
                pass  # 表还不存在 = 本来就是空库
        con.commit()
    finally:
        con.close()


def chunks(seq: list[str], n: int):
    for i in range(0, len(seq), n):
        yield seq[i:i + n]


_POOLS: dict[int, concurrent.futures.ThreadPoolExecutor] = {}


def pool_for(level: int) -> concurrent.futures.ThreadPoolExecutor:
    """模块级池（与生产实现一致：随进程常驻，不按请求重建）。"""
    if level not in _POOLS:
        _POOLS[level] = concurrent.futures.ThreadPoolExecutor(
            max_workers=level, thread_name_prefix=f"vcr-photo{level}")
    return _POOLS[level]


def classify_all(paths: list[str], registry, level: int) -> list:
    """按 8 张分块、块内 level 路并发（level=1 纯串行），返回与输入对齐的结果列表。"""
    out: list = []
    pool = pool_for(level) if level > 1 else None
    for chunk in chunks(paths, CHUNK):
        if pool is None:
            out.extend(pipeline.classify_one(p, registry) for p in chunk)
        else:
            out.extend(pool.map(lambda p: pipeline.classify_one(p, registry), chunk))
    return out


def fields(r) -> tuple:
    """等价性对比字段（person_ids 刻意除外，见模块 docstring）。"""
    if r is None:
        return ("__none__",)
    return (r.category, r.sub_category, r.label, r.confidence, r.person_count)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("root", help="真实照片目录")
    ap.add_argument("--limit", type=int, default=32)
    ap.add_argument("--rounds", type=int, default=3, help="正式计时轮数，取最快")
    ap.add_argument("--levels", default="1,2,3,4")
    ap.add_argument("--out", default="", help="报告同时写入该文件（UTF-8）")
    args = ap.parse_args()

    levels = [int(x) for x in args.levels.split(",") if x.strip()]
    if not levels or min(levels) < 1 or max(levels) > 16:
        print("[ERR] --levels 非法")
        return 2
    paths = list_photos(args.root, args.limit)
    if len(paths) < 4:
        print(f"[ERR] 照片太少: {args.root}")
        return 2

    rep: list[str] = []

    def w(s=""):
        print(s)
        rep.append(str(s))

    w("=" * 78)
    w("P-OPT2 实测：/classify_batch 照片级并行（1 线程 1 张照片）")
    w("=" * 78)
    w(f"照片目录 {args.root}")
    w(f"样本 {len(paths)} 张 · 块 {CHUNK} 张/批 · 档位 {levels} · 预热 1 轮 + 正式 {args.rounds} 轮取最快")

    registry = get_registry()
    t = time.perf_counter()
    registry.det
    registry.face_det
    registry.face_rec
    registry.ocr  # 触发惰性加载（有 RLock 保护）
    w(f"\n[0] 通道模型加载 {time.perf_counter() - t:.1f}s")
    gi = registry.gpu_info()
    w(f"    provider = {gi['provider']} | 会话绑定 = {gi['sessions']}")
    w(f"    intra_op 线程 = {config.threads()}（config.threads()）")

    # ---- 等价性：每档 reset_store 后跑一遍，逐字段对比（person_ids 允许置换） ----
    w("\n[1] 等价性验证（每档从空库出发）")
    base_fields: dict[str, tuple] = {}
    base_persons = -1
    equiv_ok = True
    for lv in levels:
        reset_store()
        rs = classify_all(paths, registry, lv)
        persons = sorted({pid for r in rs if r is not None for pid in (r.person_ids or [])})
        if lv == levels[0]:
            base_fields = {p: fields(r) for p, r in zip(paths, rs)}
            base_persons = len(persons)
            w(f"    P={lv}: 基线捕获（distinct person {base_persons}）")
        else:
            diff = [os.path.basename(p) for p, r in zip(paths, rs)
                    if fields(r) != base_fields[p]]
            status = "逐字段一致" if not diff else f"差异 {len(diff)} 张: {diff[:3]}"
            tail = "（与基线一致）" if len(persons) == base_persons else "（⚠ distinct person 与基线不同）"
            w(f"    P={lv}: {status} · distinct person {len(persons)}{tail}")
            if diff or len(persons) != base_persons:
                equiv_ok = False
    w(f"    → 等价性 {'通过' if equiv_ok else '未通过'}（person_ids 置换属预期，其余字段不得有差异）")

    # ---- 吞吐：各档预热 1 轮 + 正式 N 轮取最快，交错轮转 ----
    w("\n[2] 吞吐实测（交错轮转，取最快）")
    best: dict[int, float] = {lv: float("inf") for lv in levels}
    best_stats: dict[int, tuple[dict, int]] = {}
    for lv in levels:  # 各档先各自预热一轮（含 ORT 池行为热身）
        reset_store()
        classify_all(paths, registry, lv)
    for r in range(args.rounds):
        order = levels if r % 2 == 0 else list(reversed(levels))
        for lv in order:
            reset_store()
            t0 = time.perf_counter()
            classify_all(paths, registry, lv)
            wall = time.perf_counter() - t0
            stats, n = pipeline.take_channel_stats()  # 每轮必取（内部会清零）
            if wall < best[lv]:
                best[lv] = wall
                best_stats[lv] = (stats, n)
            w(f"    轮{r + 1} P={lv}: {wall:7.3f}s ({wall / len(paths) * 1000:7.1f}ms/张)")

    base_wall = best[min(levels)]
    w("\n" + "-" * 78)
    w(f"{'P':<4}{'最快墙钟s':>12}{'ms/张':>12}{'张/秒':>10}{'加速':>8}")
    for lv in levels:
        wall = best[lv]
        w(f"{lv:<4}{wall:>12.3f}{wall / len(paths) * 1000:>12.1f}{len(paths) / wall:>10.2f}"
          f"{base_wall / wall:>7.2f}x")
    w("-" * 78)

    # ---- 成本结构（各档最快轮的通道均摊） ----
    w("\n[3] 通道均摊（各档最快轮；并行后各通道相加 > 单张耗时，重叠部分被计了两遍）")
    for lv in levels:
        if lv not in best_stats:
            continue
        stats, n = best_stats[lv]
        per = (lambda k: stats.get(k, 0.0) / n if n else float("nan"))  # noqa: E731
        w(f"    P={lv}: decode {per('decode'):.1f} · det {per('det'):.1f} · tone {per('tone'):.1f}"
          f" · ocr {per('ocr'):.1f} · face {per('face'):.1f}(命中 {stats.get('face_used', 0.0) / n * 100:.0f}%)"
          f" · wall_gpu {per('wall_gpu'):.1f} · 等tone {per('wall_wait'):.1f}"
          f" · 合计 {per('total'):.1f}ms/张")

    w("\n结论")
    best_lv = max(levels, key=lambda lv: base_wall / best[lv])
    sp = base_wall / best[best_lv]
    w(f"1) 最优档 P={best_lv}：{sp:.2f}x（{base_wall / len(paths) * 1000:.1f} → "
      f"{best[best_lv] / len(paths) * 1000:.1f}ms/张）")
    w(f"2) 等价性 {'通过' if equiv_ok else '未通过'}（person_ids 置换属照片级并行固有语义，分组结构不变）")
    w("3) 收益来源看各档通道均摊：decode 重叠 + ONNX intra-op 缝隙填补；"
      "「等tone(wall_wait)」抬升说明 _CHANNEL_POOL 需随 P 扩容（见 pipeline.py）")

    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            f.write("\n".join(rep))
        print(f"\n[报告已写入] {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
