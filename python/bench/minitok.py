"""最小 BERT 分词器（Chinese-CLIP 文本侧，vocab.txt 驱动）

复刻 BertTokenizer：clean_text → CJK 加空格 → 空白切分 → lowercase → WordPiece 贪婪
输出 input_ids / attention_mask（[CLS]...[SEP]，[PAD]=0 填充）。
比 transformers 轻量数千倍，与方案 Phase 1「tokenizers 库/自研分词」对齐。
"""
import os
import unicodedata

PAD, CLS, SEP, UNK = "[PAD]", "[CLS]", "[SEP]", "[UNK]"


def _is_chinese_char(cp):
    return (0x4E00 <= cp <= 0x9FFF or 0x3400 <= cp <= 0x4DBF or 0x20000 <= cp <= 0x2A6DF
            or 0x2A700 <= cp <= 0x2B73F or 0x2B740 <= cp <= 0x2B81F
            or 0x2B820 <= cp <= 0x2CEAF or 0xF900 <= cp <= 0xFAFF
            or 0x2F800 <= cp <= 0x2FA1F)


class MiniBertTok:
    def __init__(self, vocab_path, max_len=52):
        self.max_len = max_len
        self.vocab = {}
        with open(vocab_path, encoding="utf-8") as f:
            for i, line in enumerate(f):
                self.vocab[line.rstrip("\n")] = i

    def _basic(self, text):
        out = []
        for ch in text:
            cp = ord(ch)
            if cp in (0, 0xFFFD) or unicodedata.category(ch) in ("Cc", "Cf"):
                out.append(" ")
            elif _is_chinese_char(cp):
                out.extend([" ", ch, " "])
            else:
                out.append(ch)
        toks = []
        for t in "".join(out).strip().split():
            t = t.lower()
            if len(t) > 100:
                t = UNK
            toks.append(t)
        return toks

    def _wordpiece(self, tok):
        pieces, start = [], 0
        while start < len(tok):
            end = len(tok)
            piece = None
            while start < end:
                sub = tok[start:end]
                if start > 0:
                    sub = "##" + sub
                if sub in self.vocab:
                    piece = sub
                    break
                end -= 1
            if piece is None:
                return [UNK]
            pieces.append(piece)
            start = end
        return pieces

    def encode(self, texts):
        """返回 (input_ids[N,L], attention_mask[N,L]) int64"""
        import numpy as np
        L = self.max_len
        ids = np.zeros((len(texts), L), dtype=np.int64)
        mask = np.zeros((len(texts), L), dtype=np.int64)
        for i, text in enumerate(texts):
            seq = [CLS]
            for t in self._basic(text):
                seq.extend(self._wordpiece(t))
            seq = seq[: L - 1] + [SEP]
            for j, tk in enumerate(seq):
                ids[i, j] = self.vocab.get(tk, self.vocab[UNK])
            mask[i, : len(seq)] = 1
        return ids, mask


if __name__ == "__main__":
    import sys
    sys.stdout.reconfigure(encoding="utf-8")
    here = os.path.dirname(os.path.abspath(__file__))
    tok = MiniBertTok(os.path.join(here, "..", "models", "chinese-clip-vit-b-16", "vocab.txt"))
    for q in ["海边日落", "一只猫", "Hello 世界", "城市夜景"]:
        ids, mask = tok.encode([q])
        inv = {v: k for k, v in tok.vocab.items()}
        n = int(mask.sum())
        print(q, "→", " ".join(inv[i] for i in ids[0, :n]))
