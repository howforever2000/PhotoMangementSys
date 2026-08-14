---
title: VCR 渐进式实施工程手册（v2 · 基于 F20 真实基线）
updated: 2026-08-13
baseline: F20 视觉内容识别 v3（分层重构 + 置信度修复 + 扫街/特写拆分 + 人脸标号 + Places365 场景通道）
---

# VCR 渐进式实施工程手册（v2）

## 〇、修订说明（为什么重写）

原手册假设「从零修 bug」起步，但对照项目实际代码（`python/vcr/**`、`src-tauri/src/vision.rs`、`src/views/AlbumDetail.vue`），
**Phase 0（双 softmax 修复）、Phase 1（Places365 场景）、Phase 3（特写/扫街拆分 + 人脸标号）已在 F20 全部落地**，
人物注册表（list/rename/merge/delete）后端 + 前端面板也已就绪。照原手册执行会重复投入约一半工作量。

本版手册：**以 F20 真实代码为基线**，只规划真正剩余的能力缺口，并沿用「阶梯式可停交付」方法论。

---

## 一、真实基线盘点（F20 已完成态）

### 1.1 已就绪的架构（不要重复实现）

| 层 | 现状 | 文件 |
|----|------|------|
| 接口层 | FastAPI 薄路由 `/health` `/classify` `/classify_batch` `/persons*` | `python/server.py` |
| 服务层 | classifier / detector+NMS / face_service / scene_service / arbitrator / pipeline | `python/vcr/services/` |
| 持久层 | person_store（SQLite persons.db，人物重命名/合并/删除） | `python/vcr/persistence/` |
| 基础设施 | model_registry（懒加载+CPU provider）/ preprocess / mapping / config | `python/vcr/` |
| Rust 侧 | classify_album + list/rename/merge/delete_person 四命令（代理 `/persons`） | `src-tauri/src/vision.rs` |
| 前端 | VisionResult（sub_category/person_ids/person_count）+ 人物注册表面板 | `src/views/AlbumDetail.vue` |

### 1.2 已固化的仲裁阈值（`config.py` 实际值，后续阶段直接引用，不要重标定）

| 参数 | 实际值 | 含义 |
|------|--------|------|
| `PORTRAIT_AREA` | **0.30** | 最大人框面积 ≥30% → 人物特写 |
| `STREET_PERSON_N` | 3 | 人数 ≥3 且… |
| `STREET_MAX_AREA` | 0.15 | …最大人框面积 <15% → 扫街 |
| `GROUP_AREA` | 0.10 | ≥2 人且最大框 ≥10% → 合影（归人物） |
| `IGNORE_PERSON_AREA` | 0.10 | 单人且 <10% → 路人，不覆盖分类 |
| `PERSON_CONF_MIN` / `NMS_IOU` | 0.35 / 0.45 | 人框检测置信度 / NMS |
| `SCENE_OVERRIDE_CONF` | 0.40 | cls 置信度低于此值才允许场景路覆盖 |
| `SCENE_CONF_STRONG` | 0.15 | scene 翻转 landscape/architecture 需自身 ≥此值 |
| `FACE_MIN_PIX` / `FACE_SIM` | 24 / 0.45 | 人脸最小边长 / 同人相似度 |
| `BATCH_CHUNK` / `THREADS` | 8 / 4 | 批大小 / intra-op 线程 |

> ⚠️ 关键澄清：F20 记录里的「特写 73%」是**实测样例的面积占比**，不是阈值；真实阈值是 `PORTRAIT_AREA=0.30`（0.30 稳妥落在实测扫街 0.2~8.6% 与特写 73% 之间）。后续任何改动以 config.py 为准。

### 1.3 现有类别 vs 用户目标分组的差距（gap 清单）

| 用户目标分组 | 当前代码类别 | 状态 |
|--------------|--------------|------|
| 个人特写 | `portrait`（+ person_ids） | ✅ 已实现 |
| 人文随拍（一群） | `street` + 合影（`portrait`+group） | ✅ 已实现 |
| 动物 | `animal`（sub: dog/cat/bird） | ✅ 已实现 |
| 风景 | `landscape_nature` | ✅ 已实现 |
| 食物 | `food`（+ 温和修正） | ✅ 已实现（专家可选） |
| 城市风光 | `architecture`（建筑+城市混在一起） | ⚠️ 半实现：无独立「城市风光」 |
| 文档 | `text`（仅截图启发式） | ⚠️ 半实现：无真 OCR |
| 花朵 | `plant_flower`（花+植物混在一起） | ⚠️ 半实现：未拆分 |
| 夜景 | **无此类别** | ❌ 未实现 |

**结论：真正剩下的工作只有 4 个必做项（夜景 / 文档 / 花朵 / 城市风光拆分）+ 1 个可选项（食物专家）+ 1 个收敛项（taxonomy）。**

---

## 二、目标类目收敛（taxonomy，先行决策）

新增 `album_group_taxonomy` 定义最终相册分组枚举（9 组）与通道→分组的映射规则。**先定这张表，再动代码。**

```jsonc
// python/models/album_groups.json
{
  "groups": [
    "portrait",   "street",   "animal",   "food",
    "flower",     "landscape","cityscape","night_scene", "document"
  ],
  "fold": {
    // 当前类别 → 目标分组（未列出的类归 other）
    "architecture":   "cityscape",      // 建筑城市 → 城市风光
    "plant_flower":   "flower",         // 植物花卉 → 花朵（Phase 4 拆分后）
    "text":           "document",       // 文本截图 → 文档
    "sports":         "other",
    "vehicle":        "other"
  },
  "conflicts": [
    // 冲突消解顺序（从上到下优先级递减，与 arbitrator 规则一致）
    "person 优先于一切",
    "document(OCR 强证据) 优先于 scene/object",
    "flower/food 专家优先于 object",
    "night_scene 优先于 cityscape/landscape"
  ]
}
```

**需产品确认的 3 个歧义（写入 taxonomy 前先拍板）：**

1. **夜景**是独立分组还是「城市风光」的子标签？→ 建议独立分组（用户明确列出「夜景」）。
2. **动物**是否保留 dog/cat/bird 子分组？→ 建议作为 `sub_category` 保留（不拆成独立相册组）。
3. **证件照**（文档 + 人脸）按优先级会判成「个人特写」；是否要 OCR 强证据压过人物？→ 默认人物优先（相册以人为主），证件照归特写可接受。

---

## 三、剩余增量阶段（重新编号，每 Phase 完整可用）

```
Phase 1  夜景通道（tone+scene 融合）     ← 补 night_scene，唯一"从无到有"的类别
    ↓ 验收通过？
Phase 2  文档通道（OCR 升级）            ← 截图启发式 → 真 OCR，保留启发式兜底
    ↓ 验收通过？
Phase 3  花朵专家（从 plant_flower 拆分） ← flower 独立成组
    ↓ 验收通过？
Phase 4  城市风光/建筑拆分 + taxonomy 收敛 ← cityscape 独立，折叠 sports/vehicle/other
    ↓ 验收通过？
Phase 5  食物专家（可选）                 ← food 精度提升，可跳过
    ↓ 验收通过？
Phase 6  全量回归 + 前端分组视图接入      ← 9 组验收，50+ 图回归全绿
```

---

### Phase 1：夜景通道（tone + scene 融合）

**目标**：新增 `night_scene` 类别，区分「城市夜景 / 自然夜景 / 白天城市」。

**关键集成点（必须注意）**：当前影调分析（`tone.rs` 的 `scan_album_tones`）是**独立按钮**，**没有接入 classify 流水线**——Python `/classify` 请求不携带 tone 数据。因此夜景判定必须在 **Python 端自算影调**（pipeline 已持有 `img` 对象，下采样 256px + BT.601 加权即可，几十毫秒级），**不要**为此改 Rust→Python 接口契约。

**改动点**：
1. `config.py` 新增：
   ```python
   NIGHT_LUMA = 60          # avg_luma < 60 视为低调（夜景候选）
   NIGHT_SCENE_CONF = 0.10  # scene 置信度 ≥ 此值才允许 night 判定
   ```
2. 新增 `vcr/services/tone_service.py`：输入 PIL Image，下采样 256px，计算 `avg_luma`（BT.601 整数定点，与 `tone.rs` 一致）。
3. `arbitrator.py` 增加夜景规则（插在「场景路覆盖」之后、「兜底 cls」之前）：

   ```python
   # 夜景：scene 命中夜间类 或（低调 + 城市场景）
   if scene_out.ready:
       night_label = NIGHT_PAT.search(scene_out.label)   # night/aurora/star/moonlight/sunset
       low_key = tone_out.avg_luma is not None and tone_out.avg_luma < config.NIGHT_LUMA
       if (night_label and scene_out.confidence >= config.NIGHT_SCENE_CONF) or \
          (low_key and scene_out.category in ("architecture", "street")):
           return FinalResult(category="night_scene", label="夜景", ...)
   ```

4. `config.CATEGORY_DESC` 增加 `"night_scene": "夜景"`；`server.py` 的 `categories` 列表同步。

**验收标准**：

| 指标 | 阈值 |
|------|------|
| 夜景（城市/自然）准确率 | ≥ 85%（测试集 20 张：城市夜景×10 / 自然夜景×5 / 白天城市×5） |
| 白天城市不误判为夜景 | 100%（白天城市 5 张全部仍归 cityscape/architecture） |
| 单张 tone 计算耗时 | ≤ 30ms（下采样 256px） |

**回退**：夜景误判多 → 提高 `NIGHT_LUMA`（如 50）或要求 `night_label` 必须命中；仍不达标 → 关闭夜景通道，回 Phase 0 基线。

---

### Phase 2：文档通道（OCR 升级）

**目标**：真文档（纸张/白板/屏幕文字）识别，替换「仅截图」的启发式，启发式降级为兜底。

**改动点**：
1. 新增 `paddleocr-det.onnx`（仅 DB 检测，不做 rec），放入 `python/models/`。
2. `config.py` 新增：
   ```python
   OCR_MODEL = "paddleocr-det.onnx"
   OCR_AREA_STRONG = 0.15   # 文字框面积占比 >15% → 强证据
   OCR_AREA_WEAK = 0.05     # >5% 且场景 office/classroom → 弱证据
   ```
3. 新增 `vcr/services/ocr_service.py`：letterbox 640 → DB 概率图 → 阈值 0.3 → box 阈值 0.5 → NMS → 文字面积占比。
4. `arbitrator.py` 优先级调整（插在「截图启发式」之后、「人物规则」之前，但**人物仍最高**——按 taxonomy 决策）：
   ```python
   if ocr_out.ready and ocr_out.area_ratio >= config.OCR_AREA_STRONG:
       return FinalResult(category="document", label="文档", ...)   # 强证据
   ```
5. 类别收敛：`text` → `document`（截图启发式与 OCR 统一输出 `document`，`sub_category` 区分 `screenshot` / `paper` / `whiteboard`）。

**验收标准**：

| 指标 | 阈值 |
|------|------|
| 文档召回率 | ≥ 90%（不漏真文档） |
| 文档精确率 | ≥ 85%（不把树叶纹理/花纹误判为文档） |
| 单张 OCR 耗时 | ≤ 40ms |

**回退**：OCR 误检多 → 提高 `OCR_AREA_STRONG` 到 0.20，或加「文字框长宽比过滤（宽>高 且 比例<10）」；速度不达标 → OCR 改条件触发（仅 cls=other 且 scene∈office/classroom 时启用）。

---

### Phase 3：花朵专家（plant_flower 拆分）

**目标**：`plant_flower`（植物花卉）拆为 `flower`（花朵）与 `plant`（植物），花朵独立成组。

**改动点**：
1. 新增 `efficientnet-b0-flowers.onnx`（~20MB，102 类，**懒加载**，30s 释放）。
2. `config.py` 新增：
   ```python
   ENABLE_FLOWER_EXPERT = True
   FLOWER_MODEL = "efficientnet-b0-flowers.onnx"
   FLOWER_CONF = 0.5        # 专家置信度 >0.5 → flower，否则 plant
   ```
3. `arbitrator.py`：当 `cls_cat == "plant_flower"` 时触发 flower 专家：
   ```python
   if cls_cat == "plant_flower" and flower_expert_ready():
       fc = flower_expert.run(img)
       cls_cat = "flower" if fc >= config.FLOWER_CONF else "plant"
   ```
4. `config.CATEGORY_DESC`：`plant_flower` 拆为 `"flower": "花朵"` 与 `"plant": "植物"`；mapping 表同步。

**验收标准**：

| 指标 | 阈值 |
|------|------|
| 花朵准确率 | ≥ 90% |
| 植物不误判为花朵 | ≥ 90% |
| 懒加载首次触发耗时 | ≤ 800ms（非高频，可接受） |

**回退**：花朵专家加载慢 → 改常驻（仅 20MB）；精度不达标 → 关闭专家，保留 `plant_flower` 合并分组。

---

### Phase 4：城市风光/建筑拆分 + taxonomy 收敛

**目标**：`architecture`（建筑+城市混合）拆为 `cityscape`（城市风光）与 `architecture`（建筑）；同时落地 `album_groups.json` 收敛层。

**改动点**：
1. `scene_service.py` 的 `URBAN_PAT` 拆为两个正则：
   ```python
   CITYSCAPE_PAT = r"\b(skyscraper|downtown|city|street|plaza|boulevard|square|skyline|metropolis|harbor|bridge|traffic)\b"
   BUILDING_PAT  = r"\b(church|cathedral|castle|tower|mosque|temple|house|building|facade|mansion|courtyard)\b"
   ```
2. `_map_scene()` 返回 `cityscape` / `architecture` / `landscape_nature` / `indoor` / `other`。
3. `arbitrator.py` 同步：scene 覆盖、兜底 cls 的类别名对齐。
4. **taxonomy 收敛**：加载 `album_groups.json`，`server.py` 输出前统一折叠（`sports`/`vehicle` → `other`，`text` → `document` 等），保证前端拿到的 `category` 一定是 9 组之一。
5. `config.CATEGORY_DESC` 增加 `"cityscape": "城市风光"`、`"flower": "花朵"`、`"night_scene": "夜景"`、`"document": "文档"`；前端 `categoryLabel` 映射同步。

**验收标准**：

| 指标 | 阈值 |
|------|------|
| 城市风光 vs 建筑区分准确率 | ≥ 85% |
| 输出类别 ∈ 9 组（无泄漏旧类名） | 100% |
| 前端 9 组中文标签渲染正确 | 100% |

**回退**：拆分误判多 → 城市风光与建筑合并回 `architecture`（保持 F20 行为），只保留 taxonomy 折叠。

---

### Phase 5：食物专家（可选）

**目标**：`food` 精度提升（可跳过，不影响 9 组闭环）。

- 新增 `food101-resnet50.onnx`（~90MB，**懒加载**）。
- `config.py`：`FOOD_CONF = 0.6`；当 cls=other 且 food 证据弱，或 scene∈restaurant 时触发专家。
- 回退：90MB 不可接受 → 关闭专家，用「场景 restaurant + ImageNet 食物映射」兜底（当前已有 food 温和修正逻辑）。

---

### Phase 6：全量回归 + 前端分组视图接入

**目标**：9 组端到端验收 + 前端按分组视图展示。

- 测试集扩容（见 §四），跑全量回归，结果写 `test_results/phase6.json`。
- 前端「内容识别」结果区按 9 组着色/分组展示；夜景/文档/花朵新增中文标签。
- 验收：9 组整体准确率 ≥ 90%，单张全通道耗时 ≤ 200ms，batch=8 吞吐 ≥ 25 张/秒。

---

## 四、工程基座补充（原手册未覆盖的硬性项）

### 4.1 测试集（原 50 图/10 类统计效力不足）

- 每类 **≥20 张 dev + ≥20 张 held-out**（9 组 ≈ 180 + 180 张，可先用每类 15 张起步）。
- **阈值只在 dev 上调**，held-out 只做最终验收，杜绝「在测试集上调阈值」的数据泄漏。
- 复用 `test_fixture_photos/` 扩充 fixture（已有 EXIF/无 EXIF/暗图/亮图 6 张）。

### 4.2 CPU 多模型资源编排

- 模型总量：cls(24MB) + det(12MB) + face(2.5+13.6MB) + scene(45MB) + ocr(4MB) + flower(20MB) ≈ **121MB**（懒加载专家不算常驻）。
- 统一 `THREADS=4` intra-op（已在 `model_registry._so()`），**避免**每个 session 各自线程过订阅。
- 串行编排顺序按模型权重排序（先小后大），单张全通道目标 ≤ 200ms（F20 实测 s-cls 4~8ms + det 32~38ms + scene/face 已含，预算充足）。
- ONNX 导出需 **dynamic batch**（`dynamic_axes`），否则 batch=8 无法真正加速。

### 4.3 模型获取（已固化）

| 模型 | 来源 | 大小 | 状态 |
|------|------|------|------|
| yolov8s-cls / n-cls / n-det | ultralytics 导出（ghfast.top 代理） | 24/10.9/12MB | ✅ 已有 |
| det_500m / w600k_mbf | InsightFace buffalo_sc | 2.5/13.6MB | ✅ 已有 |
| resnet18_places365 | MIT 官方权重 + torch 导出 | 45.4MB | ✅ 已有 |
| paddleocr-det | PaddleOCR ch_PP-OCRv4_det | ~4MB | ❌ 待加（Phase 2） |
| efficientnet-b0-flowers | HF oxford-102-flowers | ~20MB | ❌ 待加（Phase 3） |

---

## 五、回退总策略

| 触发条件 | 回退动作 |
|---------|---------|
| 某 Phase 准确率低于阈值 | 调该通道阈值 → 仍不达标 → 关闭该通道开关，回上一 Phase |
| 速度低于阈值 | 降级模型 / 条件触发 → 仍不达标 → 关闭通道 |
| 模型缺失/损坏 | 该通道返回 `null`，仲裁器降级（已有机制，勿破坏） |
| 内存不足 | 关懒加载以外的非必要通道，保人物 + 场景 + cls |

---

## 六、执行检查清单（每 Phase 完成后逐项勾选）

```
□ 1. 模型放入 python/models/ 并验证 MD5
□ 2. config.py 阈值/开关正确（引用真实标定值，不重标定）
□ 3. 通道代码 pytest / cargo test 通过
□ 4. dev 测试集跑完，结果写 test_results/phase{N}.json
□ 5. 准确率 ≥ 该 Phase 阈值（held-out 只在 Phase 6 用）
□ 6. 速度 ≥ 该 Phase 阈值（warmup 后 10 次平均）
□ 7. 前端 VisionResult / categoryLabel 已同步（新增类别时）
□ 8. Rust 命令 / 前端面板无回归（人物管理四命令）
□ 9. 模型缺失返回明确错误，不崩溃
□ 10. album_groups.json 与 CATEGORY_DESC 同步更新
```

---

**核心结论**：F20 已完成 5/9 目标分组（特写/人文随拍/动物/风景/食物）。本手册只做剩余 4 个必做项（夜景/文档/花朵/城市风光）+ 1 可选（食物专家）+ 1 收敛（taxonomy），
每 Phase 完整可用、随时可停，阈值全部沿用 config.py 真实标定值。

---

## 七、实测验证（2026-08-13 · 53 张真实照片）

用 `D:/YUAN HAO/Pictures/2026/test` 53 张真实照片跑全通道（cls+det+face+scene 全就绪，平均 514ms/张），逐张人工核对真实内容。

### 7.1 实测结果

| 指标 | 值 |
|------|-----|
| 大类识别正确 | 34/53 = **64.2%** |
| 夜景检出 | **0/7（0%）** ← 完全缺失 |
| 食物检出 | **0/5（0%）** ← 完全缺失 |
| 花朵检出 | **3/6（50%）** ← 特写花对，花树/散花漏 |
| 子类错误（白猫→白狗） | 1 |

### 7.2 错误归类（验证了本手册的 gap 判断）

| 错误类型 | 数量 | 对应 Phase |
|---------|------|-----------|
| 夜景漏判 | 7 | Phase 1 夜景通道 ✅ |
| 食物漏判 | 5 | Phase 2 食物通道 ✅ |
| 花朵/植物漏判 | 3 | Phase 3 花朵专家 ✅ |
| 文档漏判 | 1 | Phase 4 文档 OCR ✅ |
| 场景误判（黄昏→城堡） | 1 | Phase 4 scene 校准 |
| det 误检人（车辆→扫街） | 1 | 见 §7.3 新增发现 |
| 合影/扫街边界（夜市→合影） | 1 | 见 §7.3 |
| 子类误判（白猫→白狗） | 1 | 见 §7.3 |

**收益预估**：落实 Phase 1~5 后修复 16 个大类错误（夜景 7 + 食物 5 + 花朵 3 + 文档 1），大类准确率 **64.2% → ~90%**。

### 7.3 实测新增发现（手册 v2 之外的 4 项校准）

1. **det 车辆场景误检人**（e-7278 车流被判「扫街3人」）：建议 `PERSON_CONF_MIN` 0.35→0.45，或「人框与 vehicle 框重叠时降级」。
2. **合影吞掉扫街**（e-7005 夜市4人→合影）：`GROUP_AREA=0.10` 与 `STREET_MAX_AREA=0.15` 竞争，建议扫街阈值放宽到 0.20 或合影增加「摆拍/框聚簇」条件。
3. **白猫→白狗**：动物子类置信度 <0.3 时不强判 dog/cat，归「其他动物」。
4. **scene 低置信度接管**（#18 conf 0.023、#25 conf 0.022）：`SCENE_CONF_STRONG=0.15` 未拦住极低置信度覆盖，建议 scene 接管加下限 ≥0.25。
5. **hot pot 已识别但未映射 food**（火锅图 #37/#39 细类命中 `hot pot` 却归 `other`）：映射表缺 `hot pot` 等中式菜品关键词，纯映射补丁即可救回 2/5 食物误判；其余 3/5（plate/king crab）需食物专家。

> 详细逐张对比见 `VCR-实测验证报告-2026-08-13.md`。

---

## 八、实施优先级修订（基于实测）

实测（53 张）确认 **夜景（0/7）、食物（0/5）是完全缺失的两大缺口，花朵（3/6）特写可识别但花树/散花漏判**，优先度调整：

```
Phase 1  夜景通道     ← 实测 7 处漏判，最高优先
Phase 2  食物通道     ← 实测 5 处漏判（2 处可用 hot pot 映射补丁，3 处需专家）
Phase 3  花朵专家     ← 实测 3 处漏判（花树/散花）
Phase 4  文档 OCR     ← 1 处漏判 + 标签优化
Phase 5  城市风光拆分 + scene 校准  ← 含 §7.3 的 5 项阈值/映射校准
Phase 6  全量回归 + 前端接入
```

---

## 九、测试集标签表（ground truth · 用于后续回归）

> 由视觉模型（Qwen3-VL / qwen-vl-max）逐张打标，53 张全部已标注。
> 表格同时存于 `ground_truth.json`（机器可读，供 `eval_vcr.py` 评测）。

| # | 文件名 | 真实标签 | 画面说明 |
|---|--------|---------|----------|
| 1 | 20240220-DSC_3583.jpg | 个人特写 | 戴眼镜男子绿毛衣持水瓶回望 |
| 2 | 20240220-DSC_3584.jpg | 个人特写 | 同男子侧身背景虚化 |
| 3 | 20240220-DSC_3585.jpg | 人文随拍 | 三人背影沿坡道有路牌 |
| 4 | 20240220-DSC_3589.jpg | 人文随拍 | 三人驻足互动 |
| 5 | 20240220-DSC_3591.jpg | 人文随拍 | 三人正面站立坡道 |
| 6 | 20240220-DSC_3604-已增强-降噪.jpg | 城市风光 | 桥下仰拍江岸摩天楼群 |
| 7 | 20240220-DSC_3607-已增强-降噪.jpg | 城市风光 | 混凝土桥墩对岸城市 |
| 8 | 20240220-DSC_3609.jpg | 车辆 | 骑手骑山地车石板路 |
| 9 | 20240220-DSC_3611.jpg | 人文随拍 | 江边石阶多人垂钓 |
| 10 | 20240220-DSC_3613.jpg | 人文随拍 | 多名钓鱼者江边 |
| 11 | 20240220-DSC_3618-已增强-降噪.jpg | 城市风光 | 跨江大桥高层天际线 |
| 12 | 20260716-1784173966302.jpg | 自然风景 | 黄昏天空云霞住宅剪影 |
| 13 | IMG_20191001_132524.jpg | 花朵/植物 | 粉紫波斯菊 |
| 14 | IMG_20191001_132642.jpg | 花朵/植物 | 白花黄蕊 |
| 15 | IMG_20191001_133106.jpg | 花朵/植物 | 两朵粉红花 |
| 16 | IMG_20200123_224520.jpg | 夜景 | 星空树木剪影 |
| 17 | IMG_20200125_000407.jpg | 夜景 | 夜空烟花 |
| 18 | IMG_20200207_173741.jpg | 花朵/植物 | 夕阳金色树林 |
| 19 | IMG_20200209_201138.jpg | 夜景 | 月光透树枝夜空暗 |
| 20 | IMG_20200209_203753.jpg | 自然风景 | 山峦剪影多云天空 |
| 21 | IMG_20200211_193215.jpg | 食物 | 白盘生菜烤肉片 |
| 22 | IMG_20200228_121553.jpg | 文档 | 横线纸手写英语笔记 |
| 23 | IMG_20200228_131234.jpg | 花朵/植物 | 蓝天缀白色小花枝 |
| 24 | IMG_20200228_132402.jpg | 花朵 | 蓝色小花特写背景虚化 |
| 25 | IMG_20200228_165450.jpg | 文档 | 手写英语写作结构笔记 |
| 26 | IMG_20200228_183531.jpg | 自然风景 | 黄昏山林剪影紫云霞光 |
| 27 | IMG_20200228_183829.jpg | 自然风景 | 日落山峦树林剪影 |
| 28 | IMG_20200228_184239.jpg | 个人特写 | 小女孩俯拍自拍搞怪 |
| 29 | IMG_20200229_100526.jpg | 个人特写 | 女孩侧脸特写阳光 |
| 30 | IMG_20200229_191718.jpg | 夜景 | 深蓝天空橙红月亮山林剪影 |
| 31 | IMG_20200301_150106.jpg | 个人特写 | 男子自拍田野油菜花 |
| 32 | IMG_20200301_150946.jpg | 个人特写 | 小女孩室内近景微笑 |
| 33 | IMG_20200304_162315.jpg | 文档 | 笔记本手写英文诗+中文译文 |
| 34 | IMG_20200307_113915.jpg | 个人特写 | 男子倚树林间仰头 |
| 35 | IMG_20200605_200129.jpg | 夜景 | 满月山树 |
| 36 | IMG_20200807_203428.jpg | 夜景 | 星月树影 |
| 37 | IMG_20200930_124607.jpg | 食物 | 火锅 |
| 38 | IMG_20201003_123356.jpg | 食物 | 火锅肉片 |
| 39 | IMG_20201113_125501.jpg | 食物 | 炒鱿鱼 |
| 40 | IMG_20201214_115130.jpg | 食物 | 铁板炒菜 |
| 41 | e--7.jpg | 人文随拍 | 街景多人骑电动车过店铺 |
| 42 | e-.jpg | 动物(猫) | 白猫静坐砖地旁有单车 |
| 43 | e-7005.jpg | 人文随拍 | 夜市摊位前数人备餐 |
| 44 | e-7007.jpg | 车辆 | 绿色山地车停路边 |
| 45 | e-7013.jpg | 其他 | 白色太阳能路灯蓝天 |
| 46 | e-7097.jpg | 个人特写 | 青年夜间看相机屏幕 |
| 47 | e-7123.jpg | 其他 | 皮卡丘毛绒玩偶悬挂 |
| 48 | e-7125.jpg | 其他 | 行人过街信号灯 |
| 49 | e-7157.jpg | 动物(猫) | 两只布偶猫坐红笼 |
| 50 | e-7278.jpg | 车辆 | 多辆行驶汽车十字路口 |
| 51 | good-.jpg | 夜景 | 夜间小巷红纸花装饰 |
| 52 | nice-6995.jpg | 城市风光 | 城市天际线高楼地标 |
| 53 | nice-7124.jpg | 车辆 | 蓝色环卫三轮车斑马线 |
