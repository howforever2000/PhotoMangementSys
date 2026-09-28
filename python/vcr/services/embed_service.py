"""语义 embedding 服务（Chinese-CLIP 双塔，可选通道）

职责：
  - ensure()：确保当前档位拆分件就绪（首次自动拆，幂等）并经 registry 惰性加载
  - embed_text(text) / embed_texts(texts) → np.float32 (dim,) / (N,dim) 已 L2 归一化
    （语义分类：用户分类关键词 + 中性基线提示词，一次批量编码）
  - embed_images(paths) → [{path, embedding}|{path, error}]（单张错误内联）
  - current_tier() / switch_tier()：模型档位切换（B/16 ↔ L/14-336，适配不同硬件）
  - tokenizer：tokenizers 库加载 tokenizer.json；加载失败回落内置 MiniBertTok
    （BERT wordpiece，与 transformers 分词对齐，见 clip_tokenizer.py）

档位（config.CLIP_MODEL_META）差异只在路径 / dim / 输入尺寸 / 最大长度，
打分与归一化协议完全一致；切换档位必须重建语义索引（宿主侧按 model 列隔离）。
"""
import os
import threading
import time

import numpy as np

from .. import config
from ..preprocess import clip_tensor, open_image
from .. import timing

TEXT_BATCH_MAX = 64          # 单次文本批量编码封顶


# ---------------------------------------------------------------------------
# 4.1 观测：图像编码的「解码 / 推理」两段耗时累加器
#
# 动机：embed_images 是先串行解码整批、再一次性前向。单看总耗时无法判断该优化
# 解码（并行化/流水线）还是优化推理（升图优化级别/换档位）。这里分开记账，
# 由 server 的 /embed_batch 在整批返回前打印一行汇总。
#
# 纯记账：不落盘、不参与任何判断分支。
# ---------------------------------------------------------------------------
_ESTAT_LOCK = threading.Lock()
_ESTAT: dict[str, float] = {}


def _account_embed(*, decode_ms: float, infer_ms: float, n_decoded: int, n_requested: int) -> None:
    with _ESTAT_LOCK:
        _ESTAT["decode_ms"] = _ESTAT.get("decode_ms", 0.0) + float(decode_ms)
        _ESTAT["infer_ms"] = _ESTAT.get("infer_ms", 0.0) + float(infer_ms)
        _ESTAT["n_decoded"] = _ESTAT.get("n_decoded", 0.0) + float(n_decoded)
        _ESTAT["n_requested"] = _ESTAT.get("n_requested", 0.0) + float(n_requested)
        _ESTAT["calls"] = _ESTAT.get("calls", 0.0) + 1.0


def take_embed_stats() -> tuple[dict[str, float], str]:
    """取出并清空累加器 → (统计值, 可直接打印的一行摘要)。"""
    with _ESTAT_LOCK:
        s, n = dict(_ESTAT), int(_ESTAT.get("calls", 0))
        _ESTAT.clear()
    if n <= 0:
        return {}, ""
    dec, inf = s.get("decode_ms", 0.0), s.get("infer_ms", 0.0)
    tot = dec + inf
    ndec = int(s.get("n_decoded", 0))
    per_img = (dec / ndec) if ndec else 0.0
    summary = (
        f"call={n} 请求={int(s.get('n_requested', 0))}张 成功解码={ndec}张 | "
        f"解码合计 {dec:.0f}ms({(dec * 100.0 / tot if tot else 0.0):.0f}%) "
        f"推理合计 {inf:.0f}ms({(inf * 100.0 / tot if tot else 0.0):.0f}%) | "
        f"解码均摊 {per_img:.1f}ms/张"
    )
    return s, summary


class _MiniBertTok:
    """最小 BERT 分词器（tokenizers 库不可用时的兜底，与 BertTokenizer 对齐）。"""

    def __init__(self, vocab_path: str, max_len: int):
        import unicodedata

        self._unicodedata = unicodedata
        self.max_len = max_len
        self.vocab: dict[str, int] = {}
        with open(vocab_path, encoding="utf-8") as f:
            for i, line in enumerate(f):
                self.vocab[line.rstrip("\n")] = i
        self.pad_id = self.vocab.get("[PAD]", 0)

    @staticmethod
    def _is_cjk(cp: int) -> bool:
        return (0x4E00 <= cp <= 0x9FFF or 0x3400 <= cp <= 0x4DBF
                or 0x20000 <= cp <= 0x2A6DF or 0x2A700 <= cp <= 0x2B73F
                or 0x2B740 <= cp <= 0x2B81F or 0x2B820 <= cp <= 0x2CEAF
                or 0xF900 <= cp <= 0xFAFF or 0x2F800 <= cp <= 0x2FA1F)

    def encode_batch(self, texts: list[str]):
        ud = self._unicodedata
        unk = self.vocab.get("[UNK]", 100)
        ids = np.full((len(texts), self.max_len), self.pad_id, dtype=np.int64)
        mask = np.zeros((len(texts), self.max_len), dtype=np.int64)
        for i, text in enumerate(texts):
            toks = ["[CLS]"]
            # Basic：clean → CJK 加空格 → 空白切分 → lowercase
            buf = []
            for ch in text:
                cp = ord(ch)
                if cp in (0, 0xFFFD) or ud.category(ch) in ("Cc", "Cf"):
                    buf.append(" ")
                elif self._is_cjk(cp):
                    buf.extend([" ", ch, " "])
                else:
                    buf.append(ch)
            for t in "".join(buf).strip().split():
                t = t.lower()[:100]
                # WordPiece 贪婪最长匹配
                start = 0
                while start < len(t):
                    end = len(t)
                    piece = None
                    while start < end:
                        sub = t[start:end] if start == 0 else "##" + t[start:end]
                        if sub in self.vocab:
                            piece = sub
                            break
                        end -= 1
                    if piece is None:
                        toks.append("[UNK]")
                        break
                    toks.append(piece)
                    start = end
            toks = toks[: self.max_len - 1] + ["[SEP]"]
            for j, tk in enumerate(toks):
                ids[i, j] = self.vocab.get(tk, unk)
            mask[i, : len(toks)] = 1
        return ids, mask


class EmbedService:
    def __init__(self):
        self._tok = None
        self._tok_mode = ""   # fast | mini
        self._tok_key = ""    # 档位相关 key（路径+max_len），变化则重建分词器
        self._lock = threading.Lock()
        self._init_error = ""

    # ------------------------------------------------------------------
    def _paths(self) -> dict:
        return config.clip_paths()

    def tier(self) -> str:
        return config.active_clip()

    def tier_info(self) -> dict:
        p = self._paths()
        meta = config.CLIP_MODEL_META[p["name"]]
        return {
            "name": p["name"],
            "id": p["id"],
            "label": meta["label"],
            "dim": p["dim"],
            "size": p["size"],
            "accuracy": meta.get("accuracy", ""),
            "speed": meta.get("speed", ""),
            "note": meta.get("note", ""),
        }

    def _ensure_tokenizer(self):
        p = self._paths()
        key = f"{p['tokenizer']}|{p['vocab']}|{p['max_len']}"
        if self._tok is not None and self._tok_key == key:
            return
        try:
            from tokenizers import Tokenizer

            tok = Tokenizer.from_file(p["tokenizer"])
            tok.enable_truncation(max_length=p["max_len"])
            pad = tok.token_to_id("[PAD]")
            tok.enable_padding(length=p["max_len"], pad_id=pad, pad_token="[PAD]")
            self._tok, self._tok_mode, self._tok_key = tok, "fast", key
            return
        except Exception:  # noqa: BLE001
            pass
        if os.path.isfile(p["vocab"]):
            self._tok = _MiniBertTok(p["vocab"], p["max_len"])
            self._tok_mode, self._tok_key = "mini", key
            return
        raise RuntimeError("tokenizer 不可用（tokenizer.json / vocab.txt 均缺失）")

    def _encode_texts(self, texts: list[str]) -> tuple[np.ndarray, np.ndarray]:
        self._ensure_tokenizer()
        if self._tok_mode == "fast":
            encs = self._tok.encode_batch(texts)
            ids = np.asarray([e.ids for e in encs], dtype=np.int64)
            mask = np.asarray([e.attention_mask for e in encs], dtype=np.int64)
            return ids, mask
        return self._tok.encode_batch(texts)

    @staticmethod
    def _l2(v: np.ndarray) -> np.ndarray:
        return v / (np.linalg.norm(v, axis=-1, keepdims=True) + 1e-9)

    # ------------------------------------------------------------------
    def ready(self) -> bool:
        from ..model_registry import get_registry

        reg = get_registry()
        return reg.is_ready("clip_vision") and reg.is_ready("clip_text")

    def status(self) -> dict:
        from ..model_registry import get_registry

        reg = get_registry()
        info = self.tier_info()
        return {
            "clip_ready": self.ready(),
            "clip_vision_ready": reg.is_ready("clip_vision"),
            "clip_text_ready": reg.is_ready("clip_text"),
            "tokenizer": self._tok_mode,
            "error": self._init_error,
            **info,
        }

    def ensure(self) -> str:
        """确保当前档位拆分件 + 会话就绪；返回错误串（空串 = 就绪）。"""
        with self._lock:
            if self.ready():
                return ""
            from .clip_subgraph import ensure_subgraphs

            self._init_error = ensure_subgraphs()
            if self._init_error:
                return self._init_error
            from ..model_registry import get_registry

            reg = get_registry()
            _ = reg.clip_vision
            _ = reg.clip_text
            if not self.ready():
                self._init_error = reg.load_error("clip_vision") or reg.load_error("clip_text") or "会话加载失败"
            return self._init_error

    # ------------------------------------------------------------------
    def embed_text(self, text: str) -> np.ndarray:
        """中文/英文查询 → (dim,) fp32 已归一化。"""
        return self.embed_texts([text])[0]

    def embed_texts(self, texts: list[str]) -> list[np.ndarray]:
        """批量文本 → 逐条已归一化向量（语义分类关键词一次编码，避免 N 次往返）。"""
        if not texts:
            return []
        err = self.ensure()
        if err:
            raise RuntimeError(f"CLIP 未就绪: {err}")
        from ..model_registry import get_registry

        reg = get_registry()
        out: list[np.ndarray] = []
        for i in range(0, len(texts), TEXT_BATCH_MAX):
            chunk = texts[i:i + TEXT_BATCH_MAX]
            ids, mask = self._encode_texts(chunk)
            emb = reg.run_clip_text(ids, mask)     # (N, dim)
            for row in self._l2(emb):
                out.append(row)
        return out

    def embed_images(self, paths: list[str]) -> list[dict]:
        """批量图像 → 共享空间向量；单张解码失败内联错误，不拖垮整批。"""
        err = self.ensure()
        if err:
            raise RuntimeError(f"CLIP 未就绪: {err}")
        from ..model_registry import get_registry

        results: list[dict | None] = [None] * len(paths)
        pixels: list[np.ndarray] = []
        idxs: list[int] = []
        t_dec0 = time.perf_counter()
        for i, p in enumerate(paths):
            # 细粒度：PIL 解码（读文件+解 JPEG）与 CLIP 预处理（resize/crop/归一）拆开。
            # 语义腿输入是 256px 缩略图，解码占比本应很低；若这里 open 偏高，
            # 说明缩略图没命中、仍在解原图 —— 一眼可查。
            with timing.span("clip.open"):
                img = open_image(p)
            if img is None:
                results[i] = {"path": p, "error": "无法读取图片"}
                continue
            with timing.span("clip.pre"):
                pixels.append(clip_tensor(img))
            idxs.append(i)
        # 4.1 观测：解码（含 letterbox/resize/归一）与推理两段分开记，判定谁是真瓶颈
        t_dec1 = time.perf_counter()
        if pixels:
            # 批内分片 ≤8，控制 fp16 峰值内存
            with timing.span("clip.fwd"):
                out = get_registry().run_clip_vision(np.vstack(pixels))  # (N, dim)
            for row, i in enumerate(idxs):
                results[i] = {"path": paths[i], "embedding": self._l2(out[row]).tolist()}
        t_inf1 = time.perf_counter()
        _account_embed(
            decode_ms=(t_dec1 - t_dec0) * 1000.0,
            infer_ms=(t_inf1 - t_dec1) * 1000.0,
            n_decoded=len(pixels),
            n_requested=len(paths),
        )
        return results


_service: EmbedService | None = None
_service_lock = threading.Lock()


def get_embed_service() -> EmbedService:
    global _service
    with _service_lock:
        if _service is None:
            _service = EmbedService()
        return _service
