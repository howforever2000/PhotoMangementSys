# FEAT-219 参考图调色重构：三层解耦 + LUT 烘焙（方案与实施记录）

> 状态：已实施（2026-11-24）。代码、单测与真图基准均已跑通，实测见 §3.6 / §7。
> 结论：**已落地**。用「影调层 + 分区色偏层 + 同色残差层」替换现有 Reinhard 全局统计作为默认算法，
> 旧算法保留为兼容选项。全程纯 numpy、零模型、零网络，产物可烘焙 33³ LUT 并导出 `.cube`。
> 实验依据：`lab/colortransfer/`（final / overview / palette / palette2 / layered / layered2 六轮对照，
> 素材 `C:\Users\Admin\Pictures\贵阳测试` 5 张 × 参考组「清橙胶片」「日系」）。

---

## 1. 为什么要改（实测证据）

现有实现（`colortransfer.py` Reinhard 均值/方差迁移）在 100% 强度下的实测：

| 指标 | 现有 25% | 现有 100% | 本方案（E+肩部）100% |
|---|---|---|---|
| 截断像素占比 clip%（任一通道 ≥250 或 ≤5） | 0.3–2.6 | **8.6–14.5** | **0.7–3.1** |
| 高饱和区色相漂移 | 1.4–4.2° | 10.5–42.9° | 3.7–30°（整体暖化属预期） |
| 对比度保留率（P95−P5 之比） | 0.96–1.00 | **0.81–0.97** | **0.97–1.04** |
| 作用量 ΔE（相对原图） | 3.7–8.2 | 14.8–35.2 | 12.4–25.2（稳定） |

根因（对应代码位置）：

1. `reinhard_transfer_stats` 对 **L 也做均值/方差搬移** → 把参考图的曝光一起搬走，只能靠 25% 强度保命；
2. `σr/σs` **无上限** → 反差比失控（`autograde.py` 已有 `_MAX_STRETCH`/`_shoulder`/`_chroma_headroom`
   三个保护，`colortransfer.py` 一个都没有）；
3. `lab_to_rgb` 末尾硬 `clip` → 越色域部分同时改明度与色相（洋红描边）；
4. `histogram_match_sorted` 逐 RGB 通道匹配 → 通道耦合被破坏，实测蓝黄反转、饱和 ×2.74，**淘汰**；
5. `strength` 在 sRGB（gamma 域）混合 → 低强度发脏；
6. 多参考等权混池 → 色调分歧被稀释成灰（`stats_divergence` 只告警不处理）。

被否决的替代路线（均已实验）：

- **反推原图残差**（autograde 规范化 → 残差 LUT）：gray-world 白平衡把风格偏色当偏色抹掉，
  残差只剩 ΔE≈4，几乎无效果。曝光可反推，色偏不可反推。
- **主题色聚类配对**（K-means + 一对一指派 + RBF）：自由指派会「黑配白」（对比度 0.45–0.89）；
  加单调/分层约束后作用量不可控（同一参考组对不同素材 ΔE 2.7–19.4）；锚点稀疏导致色带
  （5bit 色数 0.47–0.85）。**不作默认**，留作后续「手动锚点」高级模式（独立 FEAT）。
- **生成式重绘 / 深度模型**：改身份、改细节、丢分辨率，与「只调不改」定位冲突
  （见 `autograde.py` 模块头设计原则）。本方案 0 模型。

---

## 2. 方案总览

```
参考图(≤10张) ──► RefProfile（一次构建，LRU 缓存）
                    ├─ 聚类剔除分歧参考（§3.6）
                    └─ 池化 Lab 样本（长边1024 → 抽样 ~90k px/张）

每张素材 ──► fit_mapping(src, profile)        ← 只看统计，~0.3–0.6s
              ├─ A1 影调曲线（分位对齐 + 曝光锚定）
              ├─ A2 分区色偏 Δa(L), Δb(L)（10 带）
              └─ A3 同色残差网格（12 色相 × 3 影调）
                    │
                    ▼
            eval_mapping(rgb) —— 纯逐色函数（无空间运算）
              A4 肤色门控 → A5 色域回收 → Lab 混合 strength → A6 亮度肩部
                    │
                    ▼
            build_lut(33³, 轴序[b,g,r]) ──► lutcube.apply_cube（整批查表）
                    └─ 可选导出 .cube（冻结首张映射，§3.8）
```

三个「层」各管一件事，互不越权：

| 层 | 管什么 | 轴 | 权重默认 | 限幅 |
|---|---|---|---|---|
| A1 影调 | 曲线**形状**（黑位/中间调/高光），不搬曝光 | L 分位 | toneWeight 0.85 | ΔL ±22 |
| A2 色偏 | 整体风格（暖高光/冷阴影） | L 分带（内容无关） | chromaWeight 1.0 | ‖Δab‖ ≤30 |
| A3 残差 | 同色换色（蓝→青蓝） | 色相×影调（内容相关） | 0.45（内部常量） | Δab ≤12，ΔL ≤12 |

---

## 3. 算法规格（可直接照写代码）

新模块 `python/ai_studio/refgrade.py`，纯函数、无状态、只依赖 numpy。
Lab↔RGB 复用 `colortransfer.rgb_to_lab / lab_to_rgb` 的矩阵常量（**禁止另抄矩阵**，
`autograde._lab_to_linear` 的注释记录了漏 `.T` 的事故）；色域回收复用
`autograde._chroma_headroom`（二分 6 次，`_GAMUT_EPS=1e-5`）。

### 3.1 A1 影调层：分位曲线 + 曝光锚定

输入：src 的 L 样本、ref 池的 L 样本（各 ~200k / ~90k×N）。

```
KNOTS = [2, 10, 25, 50, 75, 90, 98]          # 分位点（%）
qs = percentile(src_L, KNOTS);  qr = percentile(ref_L, KNOTS)
d  = clip(qr - qs, -22, +22)                # 每个分位的位移，限幅
d  = maximum.accumulate(d)                  # 强制单调 → 曲线永不反转影调
xp = [0] + qs + [100]
fp = [d[0]*0.5] + d + [d[-1]*0.5]           # 端点斜率减半，避免 0/100 台阶
xp 去重（保序取首索引）
dL(L) = interp(L, xp, fp)
if keepExposure:                            # 默认 True
    dL -= interp(median(src_L), xp, fp)     # 锚定：只搬形状，不搬位置
dL *= toneWeight                            # 默认 0.85
```

- 验收：`keepExposure=True` 时输出中位 L 相对原图偏移 ≤2；
  合成亮度斜坡输入，输出 L 必须单调不减（防负反差）。
- 实验读数：锚定使 100% 强度 clip 从 20.1% → 13.2%（肩部前）。

### 3.2 A2 分区色偏层：按 L 分带的 Δa/Δb

```
10 个等宽 L 带（0–100，步长 10），band_centers = [5,15,...,95]
对 src / ref 池分别求每带 (mean_a, mean_b)
空带：沿带轴线性插值补齐；再沿带轴 (1,2,1)/4 平滑一次
Δa(L) = interp(L, centers, ref_a - src_a) * chromaWeight
Δb(L) 同理
n = hypot(Δa, Δb);  k = min(1, 30 / n);  Δa *= k; Δb *= k     # 向量限幅 30
```

- 分带轴是**明度**而非内容 → 两张照片题材完全不同也不会认错物件；
  暖高光/冷阴影的分离调色天然落在这层。
- 这层是「整体风格」的主要来源，**允许**大色相偏移（整体暖化时高饱和区漂 20–80° 属预期，
  验收用盲测而非色相硬上限）。`chromaWeight` 滑杆是用户的收敛手段。

### 3.3 A3 同色残差层：12 色相扇区 × 3 影调层

拟合（fit 阶段，src/ref 各 ~200k 样本）：

```
色相：h = atan2(b,a) ∈ [0,360)，12 扇区（30°/区），硬分箱 floor(h/30)
影调：硬分箱 L<33 / 33–66 / >66（仅用于统计分箱）
格子 (hue,zone) 内：delta = mean_lab(ref) - mean_lab(src)
有效格：两侧像素数均 ≥ 0.002 × N_src
空格外推：60 轮邻域扩散（色相轴循环、影调轴 edge-pad），
          只填「未知且邻居已知」的格，逐轮传播
平滑：色相轴循环 (1,2,1)/4 ×3；影调轴 edge-pad (1,2,1)/4 ×2
限幅：‖Δab‖ ≤12；ΔL ∈ [-12,12]
```

应用（eval 阶段，逐色）：

```
影调软权重：三角核，centers=[16.5,49.5,83]，半宽 33.5，归一化（partition of unity）
色相软权重：相邻两扇区线性插值（floor + frac）
d = Σ hue_w × zone_w × delta[hue,zone]      # 矩阵乘 (N,36)@(36,3)
彩度门控：gate = max(C/6, 0.3)             # 近中性像素少动，防灰底染色
d *= gate * 0.45                           # local_w=0.45
```

- **软权重混合是硬要求**：硬分层在层边界留台阶（实验 layered 硬版 5bit 色数 0.61 vs 软版 0.71）。
- 参考图没有的颜色 → 空格外推自邻居（「蓝→参考图的青蓝」），外推不到就接近 0（保持原样）。
  UI 需提示「参考图未覆盖的颜色已保持原样」，避免用户误以为没生效。

### 3.4 A4 肤色门控（v1 纯色函数，零模型）

```
skin = (20° ≤ h ≤ 60°) & (8 ≤ C ≤ 45) & (35 ≤ L ≤ 95)   # 输入色的 LCh
g = 1 - 0.65 * smooth(skin)      # skin 边界用 ±5 的软过渡，避免断层
A2 的 (Δa,Δb) 与 A3 的 (d_a,d_b) 乘 g；A1 影调不受影响
```

- 逐色函数 → **可烘焙进 LUT**，`.cube` 与运行时行为一致。
- 参数 `protectSkin`：代码默认 False；开关已在 UI 暴露，等真实人像基准（肤色卡+逆光/暖光人像）
  通过后再翻默认 True。当前单测已验证「肤色区色偏至少压掉一半」。
  后续可升级为人脸 mask（`ops_local._analyze_faces` 现成），
  但那是空间操作，只能做 LUT 后混合、不进 `.cube`——本期不做。

### 3.5 A5 色域回收 + Lab 混合

```
tgt_L = L + dL + d_L(local)
tgt_a = a + Δa + d_a(local);  tgt_b = b + Δb + d_b(local)
cf = min(1, _chroma_headroom(tgt_lab))       # 只缩 C，不改 h/L
tgt_a *= cf; tgt_b *= cf; tgt_L = clip(tgt_L, 0, 100)
out_lab = src_lab * (1-strength) + tgt_lab * strength    # 混合在 Lab，不在 sRGB
rgb = lab_to_rgb_float(out_lab)              # 0..1 浮点，末尾 clip
```

### 3.6 A6 高光肩部（已 A/B 定稿）

现状问题：`autograde._shoulder` 是**逐通道** smoothstep（knee=0.86 → target=0.97）。
三通道压缩量不同 → 高饱和亮部被拉向灰白（实验观察到天空变奶油色）。

定稿规格（仍是纯逐色函数）：

```
Y = 0.2126R + 0.7152G + 0.0722B            # gamma 域 Rec709，与 autograde._luma 同式
if Y > knee(0.88):
    f = shoulder_smoothstep(Y; knee=0.88, target=0.97) / Y
    rgb *= f                                # 等比缩放 → 色相/饱和比例不变
通道兜底：对 ≥0.95 的通道再做一次窄肩部（knee=0.95, target=0.97）
```

- knee 从 0.86 提到 0.88：只压顶端 12%，减少「整片天空进肩部」。
- **通道兜底为什么从「>1.0」改成「≥0.95」**：实测只判 >1 完全无效——
  `_linear_to_srgb` 已经把越色域通道裁成 1.0；而清橙组大量 250–254 的贴顶像素
  仍被 clip% 计入（ratio 版首轮 15.7%）。0.95 起的窄肩部把最大值压到 247
  （0.97×255），clip 立刻回到 1.4–2.2%，且只影响最顶端 5% 的通道。
- 实测 A/B（贵阳测试 2 张 × 两组参考，100% 强度）：

| 组/素材 | 肩部 | clip% | 对比度 | ΔE | 高光彩度比 |
|---|---|---|---|---|---|
| 清橙 / 175422 | ratio | 1.42 | 1.034 | 20.2 | 0.385 |
| 清橙 / 175422 | channel | 1.42 | 1.039 | 20.0 | 0.376 |
| 清橙 / 180153 | ratio | 2.21 | 1.025 | 24.7 | 1.155 |
| 清橙 / 180153 | channel | 2.21 | 1.027 | 24.6 | 1.135 |
| 日系 / 175422 | ratio | 1.86 | 1.024 | 13.1 | 0.286 |
| 日系 / 175422 | channel | 1.86 | 1.022 | 13.1 | 0.258 |
| 日系 / 180153 | ratio | 1.53 | 0.987 | 12.0 | 0.407 |
| 日系 / 180153 | channel | 1.53 | 0.983 | 12.0 | 0.367 |

- 结论：两者 clip/对比度/ΔE 同级，**ratio 的高光彩度比在四组里全部不低于逐通道版**，
  且色相不被逐通道压歪，定稿 `shoulder_mode="ratio"`；逐通道版保留为
  `shoulder_mode="channel"` 的回退开关（不进 UI）。
- 真实照片的高光彩度比不到 0.90，是因为掩膜里包含大片近中性天空（C 本身接近 0），
  不是肩部失效；纯色/暖高光合成用例（单测 `test_shoulder_preserves_chroma`）仍要求 ≥0.90。
- 验收：knee 以下像素逐字节不变；clip% ≤3.5；ratio 的高光彩度比不低于 channel；
  目视清橙组天空不得出现奶油色发灰。

### 3.7 多参考聚类（替换等权混池）

```
每张参考 → 10 带 (mean_a, mean_b) 向量（§3.2 同式）
两两距离 = 各带 ‖Δab‖ 的均值
并查集聚类，阈值 12（可配 clusterThreshold）
取最大簇（平局取先选者）；簇外参考剔除并告警：
  「参考图 X 与主色调差异过大，已不参与共性提取」
簇内参考的 Lab 样本直接拼接成池（簇内色调一致，混池安全）
```

- 单张参考：直通，零开销。
- 现有 `_DIVERGENCE_WARN=20` 的全局均值告警保留（兼容提示），新聚类是实际处理。
- 验收：2 暖 + 1 冷参考 → 冷图被剔除且有告警；3 张同族 → 全保留。

### 3.8 LUT 烘焙、缓存与 `.cube` 语义（重要设计决策）

**映射依赖素材统计**（分位曲线/带均值/残差网格都含 src 侧），因此：

- `RefProfile`（参考侧样本 + 聚类结果）**一次构建、整批复用**（LRU，键=路径+mtime+size，
  与 `ops_local._REF_CACHE` 同式）；
- `fit_mapping` **每张素材各拟合一次**（~0.3–0.6s，只看抽样统计）；
- `build_lut(33³)` 每张各烘焙一次，`lutcube.apply_cube` 查表（12MP ≈0.6s）。
  跨图 LUT 缓存命中率低是**预期行为**，不要为省这一步把 src 统计从映射里剥掉
  （剥掉就退化成「反推原图」路线，已证伪）。
- `.cube` 导出：**冻结第一张素材的映射**，文件名后缀沿用 `参考图调色_{ref名}.cube`，
  `meta` 增加 `cubeFrozenFrom: <首张文件名>`；文档/UI 注明「导出滤镜按首张素材定标，
  套用到其他照片时影调锚定可能略有偏差」。这与旧版「按合并统计导出」的近似语义同级，
  但必须显式告知。
- 轴序 **[b,g,r]**，写出走 `colortransfer.cube_text_from_lut`（0..1 浮点），
  读取 `lutcube.load_cube`，往返互逆（`local-enhance.md §1` 的红蓝互换事故不得复发）。

---

## 4. 工程改动清单（文件级）

| # | 文件 | 改动 | 说明 |
|---|---|---|---|
| 1 | `python/ai_studio/refgrade.py` | **新增** | §3 全部纯函数；不 import PIL/工作流 |
| 2 | `python/ai_studio/workflows/color_transfer.py` | 修改 | method 分发加 `adaptive`（新默认）；RefProfile 一次构建；process 改 fit→build_lut→apply_cube；exportCube 走首张 LUT；meta 增加 toneWeight/chromaWeight/keepExposure/protectSkin/cubeFrozenFrom |
| 3 | `python/ai_studio/ops_local.py` | 修改 | `_ref_artifact` 支持 `"adaptive"` → RefProfile；`op_color_transfer` 加 adaptive 分支与新参数（snake_case：`tone_weight`/`chroma_weight`/`keep_exposure`/`protect_skin`） |
| 4 | `python/ai_studio/op_catalog.json` | 修改 | 两处 `color_transfer`：链条步（~L308）values 加 `adaptive`，**default 保持 `reinhard`**（旧配方重跑零回归）；工作流（~L543）values 加 `adaptive`，default 改 `adaptive`；params 增补新滑杆/开关 |
| 4b | `python/ai_studio/recipecatalog.py` | 修改 | `TUNABLE_PARAMS["color_transfer"]` 放行 `tone_weight/chroma_weight/keep_exposure/protect_skin`（FEAT-218 同款口径，否则配方覆盖被拒） |
| 5 | `src/utils/aiPresets.ts` | 修改 | `CT_METHODS` 首位加 `{value:"adaptive", label:"智能分层", hint:"影调/色偏/同色三层解耦，100% 强度可用（推荐）"}`；reinhard 改 label「经典统计」，histogram 标「（不推荐）」 |
| 6 | `src/components/workshop/ai/ColorTransferTool.vue` | 修改 | method 默认 `adaptive`；strength 默认 100；adaptive 下显示「影调强度 toneWeight=85」「色调强度 chromaWeight=100」+ 开关「保持曝光」（默认开）「保护肤色」（M4 后默认开）；reinhard 下保留旧「明度迁移 lWeight」；options 透传新字段 |
| 7 | `src-tauri/src/ai_studio.rs` | **零改动（已确认）** | options 为透传 JSON（`pub options: serde_json::Value`）；前端 build 已通过 |
| 8 | `python/tests/test_refgrade.py` | **新增** | §6 测试清单 |
| 9 | `python/bench/grade_ref_eval.py` | **新增** | 指标+拼图基准（从 lab 实验移植，公式见附录 A） |
| 10 | `feature-notes.json` | 修改 | 新增 FEAT-219 条目（当前最大 id 为 FEAT-218，已核实） |
| 11 | `docs/FEAT-219-参考图调色三层解耦.md` | 本文档 | 已补实施结果、实测指标与 A/B 结论 |

**不改**：`colortransfer.py` 全部现有函数（reinhard/histogram/merge/cube 写出原样保留，
旧路径逐字节回归）；`autograde.py`（只复用，不修改）；`lutcube.py`。

### 4.1 refgrade.py 函数签名

```python
@dataclass(frozen=True)
class RefProfile:
    samples_lab: np.ndarray        # (N,3) 簇内池化 Lab 样本
    band_ab: np.ndarray            # (10,2) 池化带均值
    kept: tuple[int, ...]; dropped: tuple[int, ...]

@dataclass(frozen=True)
class Mapping:
    tone_xp: np.ndarray; tone_fp: np.ndarray      # A1（已含锚定与 toneWeight）
    band_da: np.ndarray; band_db: np.ndarray      # (10,) A2（已含 chromaWeight/限幅）
    residual: np.ndarray           # (12,3,3) A3（已含平滑/限幅/0.45）
    strength: float; protect_skin: bool
    shoulder_mode: str             # "ratio"（默认）或 "channel"（回退）

def build_profile(refs_u8: list[np.ndarray], cluster_threshold: float = 12.0) -> RefProfile
def fit_mapping(src_u8: np.ndarray, profile: RefProfile, *,
                tone_weight: float = 0.85, chroma_weight: float = 1.0,
                keep_exposure: bool = True, protect_skin: bool = False,
                strength: float = 1.0, shoulder_mode: str = "ratio") -> Mapping
def eval_mapping(m: Mapping, rgb01: np.ndarray, strength: float | None = None) -> np.ndarray
def build_lut(m: Mapping, size: int = 33) -> tuple[np.ndarray, int]  # 轴序 [b,g,r]
def transfer(src_u8, profile, *, strength=1.0, **fit_kw) -> tuple[np.ndarray, np.ndarray, int]
    # 便捷入口：fit → build_lut → lutcube.apply_cube；返回 (out_u8, lut, n)
```

strength 在 `eval_mapping` 内做 Lab 混合（烘焙进 LUT），`apply_cube` 一律 strength=1.0，
避免二次混合（`autograde.grade` 同款纪律）。

### 4.2 参数表与兼容映射

| 参数（工作流 camelCase / 链条 snake_case） | 范围 | 默认 | 旧参数兼容 |
|---|---|---|---|
| method | adaptive / reinhard / histogram | 工作流 adaptive；链条 reinhard | 缺省旧值走旧路径，逐字节不变 |
| strength | 0–1 | 1.0 | 同旧 |
| toneWeight / tone_weight | 0–1 | 0.85 | 旧 `lWeight` 仅在 reinhard 下生效；adaptive 收到 lWeight 时忽略并在 meta 记 `ignoredParams` |
| chromaWeight / chroma_weight | 0–1 | 1.0 | 新增 |
| keepExposure / keep_exposure | bool | True | 新增 |
| protectSkin / protect_skin | bool | False（开关已可用；人像基准后再翻 True） | 新增 |
| exportCube | bool | False | 语义变化见 §3.8 |
| clusterThreshold | 4–30 | 12 | 新增（暂不上 UI） |

---

## 5. 性能预算

| 阶段 | 预算 | 实测（贵阳测试 12MP） |
|---|---|---|
| RefProfile（3 张参考，长边 1024） | ≤0.5s，整批一次 | 70–88ms（10 张约 0.25s） |
| fit_mapping（每张，RGB 域先抽 200k 再转 Lab） | ≤0.6s | 约 0.2–0.35s |
| build_lut 33³（35937 色 × headroom 二分 6 次） | ≤0.6s | 约 0.2–0.3s |
| apply_cube 12MP | ≈0.6s | 约 0.5–0.6s（另加首次 97³ 稠密表 ~0.87s） |
| **单张合计（含建表）** | **≤2s** | **1.06–1.27s** |
| 8 张批量（照片级并行，`run_batch`） | ≤4s（8 线程） | 纯 CPU，GIL 释放充分（工作流头注释） |

回归门槛：单张超过 3s 即阻塞（预算 2s + 50%）。实测已留出约 0.7s 余量；
若要进一步省，优先降 fit 抽样 200k→100k（ΔE 变化需重验）。

---

## 6. 测试清单（python/tests/test_refgrade.py，unittest 风格对齐 test_colortransfer_multi.py）

1. **恒等**：strength=0 → 与输入逐字节一致；toneWeight=0 且 chromaWeight=0 且残差全零格 → 仅肩部路径，knee 以下逐字节一致。
2. **影调单调**：合成 L 斜坡（0→100）输入，输出 L 单调不减；keepExposure=True 时中位 L 偏移 ≤2。
3. **分层无台阶**：合成亮度斜坡 + 固定色度，输出 5bit 唯一色数比 ≥0.9，相邻行 ΔL 无 >3 的孤立跳变。
4. **色域回收**：sRGB 六原色 + 高饱和色卡输入，输出全部在 [0,1]，色相角变化 ≤0.5°（headroom 只缩 C）。
5. **肩部**：knee 以下逐字节不变；等比版在暖高光合成用例上彩度比 ≥0.90；0.95 窄通道兜底把贴顶通道压到 247 以下。
6. **LUT 等价**：eval_mapping 直算 vs build_lut+apply_cube，全图平均通道误差 ≤1.5（33³ 插值；实测约 0.5）；`cube_text_from_lut → load_cube` 往返逐值一致（轴序 [b,g,r]）。
7. **多参考聚类**：2 暖 + 1 冷 → 冷被剔除且 dropped 非空；单张直通；同族 3 张全保留。
8. **肤色门控**：标准肤色块（L=65,C=25,h=40°）在 protectSkin=True 下色度位移 ≤ 关闭时的 40%；非肤色块不受影响。
9. **旧路径零回归**：reinhard / histogram 对固定合成图输出与改动前 golden 哈希一致（改动前先录哈希）。
10. **限幅**：任意输入下单像素 ‖Δab‖（A2+A3 合计）≤42、|ΔL| ≤22×toneWeight+12×0.45。

## 7. 基准与验收（python/bench/grade_ref_eval.py）

数据集：`贵阳测试` 5 张 × {清橙胶片, 日系}（各取 3 张：01/04/07 与 03/06/09，与实验同口径）。
指标公式见附录 A。输出对比拼图 + metrics.json 到 `lab/colortransfer/feat219/`。

**客观门槛（adaptive 100%）**：

- clip% ≤3.5（实测 1.42–2.21；原图自身 7.8–8.0 是天空亮部基线，算法增量全为负）；
- 对比度保留 ∈[0.95,1.05]（实测 0.987–1.039）；
- 作用量 ΔE ∈[8,26] 且同组参考跨素材极差 ≤12（实测 12.0–24.7，清橙组极差 4.5、日系组 1.1）；
- 5bit 色数比 ≥0.70（实测 0.735–0.938；肩部必然压缩顶端 5% 的色阶，0.75 的初版门槛过紧）、
  梯度能量比 ∈[0.9,1.25]（实测 1.07–1.21）；
- 肤色卡：protectSkin=True 时肤色 ΔE ≤ 关闭时 50%（单测通过）。

**实测（2026-11-24，`lab/colortransfer/feat219/metrics.json`）**：

| 组 / 素材 | clip% | 对比度 | ΔE | SWD | 5bit 色数 | 梯度 |
|---|---|---|---|---|---|---|
| 清橙 / 175422 | 1.42 | 1.034 | 20.2 | 0.102 | 0.735 | 1.207 |
| 清橙 / 180153 | 2.21 | 1.025 | 24.7 | 0.125 | 0.913 | 1.069 |
| 日系 / 175422 | 1.86 | 1.024 | 13.1 | 0.068 | 0.938 | — |
| 日系 / 180153 | 1.53 | 0.987 | 12.0 | 0.060 | 0.857 | — |

目视：清橙组两张均为暖橙胶片感，高光云层保层次、阴影未压死；日系组保留冷青天空，
没有出现主题色法那种粉紫天空或色带。清橙 / 180153 的高饱和区色相漂移 67.7°，
属于「整幅暖化」的预期结果（参考图本身是强暖调），不是跨色相族嫁接；若盲测认为过火，
用「色调强度」滑杆收敛即可。

**盲测门槛（待用户执行）**：20 对（新 100% vs 现有 25%），「更像参考图」与
「更愿意保留这张照片」双问，新方案偏好率 ≥70% 视为通过。代码已默认 adaptive，
盲测若不过，前端默认一行改回 `reinhard` 即可回滚。

**目视硬边界**：色环图无扇区断层；亮度斜坡无横带；清橙组天空无奶油发灰（§3.6）；
无洋红描边（放大 200% 检查高饱和边缘）。

---

## 8. 里程碑

| 阶段 | 内容 | 产出 | 状态 |
|---|---|---|---|
| M1 | refgrade.py（A1–A5）+ 单测 + 基准移植 | 复现实验指标 | ✅ 完成（15 项单测全绿） |
| M2 | A6 肩部 A/B（ratio vs channel）+ 测试 5 + 目视 | 肩部定稿 | ✅ 完成，定稿 ratio + 0.95 窄兜底 |
| M3 | 工作流/链条/catalog/前端接线 + 兼容映射 + `.cube` 语义 + Rust 透传冒烟 | 端到端可跑，旧配方零回归 | ✅ 完成（vue-tsc / json / 旧 15 项回归通过） |
| M4 | 多参考聚类 + 肤色门控 + 测试 7/8 + 人像基准 | 聚类与肤色验收 | ✅ 代码与单测完成；`protectSkin` 默认仍 False（等真实人像基准再翻） |
| M5 | 盲测 20 对 + feature-notes FEAT-219 + 文档实测 | 发布 | ✅ 记录/文档完成；盲测待用户执行 |

实际用时约 1 个工作日（代码与验证一次跑通）。M1/M2 未改任何现有文件；
M3 起才触碰工作流与前端；`colortransfer.py` / `autograde.py` / `lutcube.py` 全程零改动。

## 9. 风险与回滚

| 风险 | 等级 | 缓解 / 回滚 |
|---|---|---|
| 高光肩部把亮部拉灰 | 低 | 已 A/B：ratio 四组高光彩度均不低于 channel；0.95 窄兜底只压最顶端 5% 通道；若仍不满可设 `shoulder_mode="channel"` |
| 0.95 兜底压掉顶端色阶 | 低 | 实测 5bit 色数 0.735–0.938、梯度 1.07–1.21；清橙 175422 为 0.735，若盲测嫌平可把兜底 knee 抬到 0.96（clip 需重验） |
| 整体暖化被用户当「偏色」 | 中 | chromaWeight 滑杆 + 盲测门槛；UI 文案说明「色调层会整体偏色，属风格迁移」 |
| 每张 LUT 烘焙拖慢批量 | 低 | 预算 §5；超标的降级路径：fit 抽样 200k→100k、LUT 33³→25³（需重验 max ΔE） |
| `.cube` 冻结首张语义引起困惑 | 低 | 文件名/meta/UI 三处显式标注（§3.8） |
| 旧配方/旧调用回归 | 低 | 链条 default 保持 reinhard；旧函数零改动；golden 哈希测试 |
| 一键回滚 | — | 前端 method 默认改回 `reinhard` + catalog 工作流 default 改回（两行）；refgrade.py 为纯新增模块，可整体摘除 |

## 10. 非目标（本期不做）

- 主题色/锚点手动模式（独立 FEAT，交互原型见 `lab/colortransfer/palette2` 结论）；
- 人脸 mask 级肤色保护（需模型/空间操作，破坏 LUT 纯度）；
- 成对照片学习（原片+成片 → 回归 LUT，质量上限路线，另立项）；
- 任何生成式/深度模型路径。

---

## 附录 A：基准指标公式（从 lab 实验移植）

- `clip%`：任一通道 ≥250/255 或 ≤5/255 的像素占比×100。**须同时报告原图自身 clip%**
  （贵阳素材天空自带 7.7–8.0%），算法增量 = 输出 − 原图。
- `hueDrift`：原图 C>20 的像素上，|wrap(h_out−h_src)| 按 C 加权均值（度）。
- `satRatio`：mean(C_out)/mean(C_src)。
- `contrast`：(P95−P5)(L_out) / (P95−P5)(L_src)。
- `dE_fromSrc`：mean(‖Lab_out−Lab_src‖₂)。
- `swd`：Lab 按 [100,60,60] 归一后，12 个随机单位方向投影的 1D 分位（64 点）绝对差均值，
  再对方向取均值（切片 Wasserstein 近似，seed=0）。
- `grad`：亮度一阶差分绝对值均值之比（out/src）。
- `colors`：5bit 量化（×31 取整）后唯一 RGB 数之比（out/src）。
- 拼图：4 列网格，行高按该行最高缩略图，标签用 msyh.ttc；参考组并排灰底展示。

## 附录 B：实验资产索引

- `lab/colortransfer/final/`：现有 25%/100% vs E+肩部 100% 终版对照 + metrics.json（本方案直接依据）
- `lab/colortransfer/overview/`、`v2/`、`v3/`：E 的锚定/肩部消融
- `lab/colortransfer/palette/`、`palette2/`：主题色法反例与约束版（非目标依据）
- `lab/colortransfer/layered/`、`layered2/`：影调分层×层内配色（软/硬分层对照、同族门限）
- 实验脚本在会话工作区 `ctexp/run*.py`（会话级，不随仓库）；**M1 移植基准时以附录 A 公式为准重写**，
  不依赖会话文件。
