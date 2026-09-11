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

import numpy as np

from .. import config
from ..preprocess import clip_tensor, open_image

TEXT_BATCH_MAX = 64          # 单次文本批量编码封顶


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
        for i, p in enumerate(paths):
            img = open_image(p)
            if img is None:
                results[i] = {"path": p, "error": "无法读取图片"}
                continue
            pixels.append(clip_tensor(img))
            idxs.append(i)
        if pixels:
            # 批内分片 ≤8，控制 fp16 峰值内存
            out = get_registry().run_clip_vision(np.vstack(pixels))  # (N, dim)
            for row, i in enumerate(idxs):
                results[i] = {"path": paths[i], "embedding": self._l2(out[row]).tolist()}
        return results


_service: EmbedService | None = None
_service_lock = threading.Lock()


def get_embed_service() -> EmbedService:
    global _service
    with _service_lock:
        if _service is None:
            _service = EmbedService()
        return _service
