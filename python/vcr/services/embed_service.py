"""语义 embedding 服务（Chinese-CLIP ViT-B/16 fp16，可选通道）

职责：
  - ensure()：确保双塔拆分件就绪（首次自动拆，幂等）并经 registry 惰性加载
  - embed_text(text) → np.float32 (512,) 已 L2 归一化
  - embed_images(paths) → [{path, embedding}|{path, error}]（单张错误内联，仿 classify_batch）
  - tokenizer：tokenizers 库加载 tokenizer.json；加载失败回落内置 MiniBertTok
    （BERT wordpiece，与 transformers 分词对齐，见 clip_tokenizer.py）
"""
import os
import threading

import numpy as np

from .. import config
from ..preprocess import clip_tensor, open_image


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
        self._tok_mode = ""  # fast | mini
        self._lock = threading.Lock()
        self._init_error = ""

    # ------------------------------------------------------------------
    def _ensure_tokenizer(self):
        if self._tok is not None:
            return
        try:
            from tokenizers import Tokenizer

            tok = Tokenizer.from_file(config.CLIP_TOKENIZER_JSON)
            tok.enable_truncation(max_length=config.CLIP_MAX_LEN)
            pad = tok.token_to_id("[PAD]")
            tok.enable_padding(length=config.CLIP_MAX_LEN, pad_id=pad, pad_token="[PAD]")
            self._tok, self._tok_mode = tok, "fast"
            return
        except Exception:  # noqa: BLE001
            pass
        if os.path.isfile(config.CLIP_VOCAB):
            self._tok, self._tok_mode = _MiniBertTok(config.CLIP_VOCAB, config.CLIP_MAX_LEN), "mini"
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
        return {
            "clip_ready": self.ready(),
            "clip_vision_ready": reg.is_ready("clip_vision"),
            "clip_text_ready": reg.is_ready("clip_text"),
            "tokenizer": self._tok_mode,
            "error": self._init_error,
        }

    def ensure(self) -> str:
        """确保拆分件 + 会话就绪；返回错误串（空串 = 就绪）。"""
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
        """中文/英文查询 → (512,) fp32 已归一化。"""
        err = self.ensure()
        if err:
            raise RuntimeError(f"CLIP 未就绪: {err}")
        from ..model_registry import get_registry

        ids, mask = self._encode_texts([text])
        out = get_registry().run_clip_text(ids, mask)  # (1,512)
        return self._l2(out[0])

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
            # 批内分片 ≤8，控制 fp16 峰值显存/内存
            out = get_registry().run_clip_vision(np.vstack(pixels))  # (N,512)
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
