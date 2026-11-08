"""脱敏与摘要：密钥不进日志这条硬约束的回归测试（docs/v5.md §8）。

为什么值得单独测：`redact()` 是"写日志前最后一道闸"，它一旦漏掉某个键名，
泄漏是**静默**的 —— 日志照常写，没人会发现里面有 sk-。所以这里逐类断言。
"""
from __future__ import annotations

from agent.logutil import mask_secret, prompt_digest, redact


def test_redact_masks_nested_secret_keys():
    body = {
        "jobId": 17,
        "provider": {"baseUrl": "https://api.example.com/v1", "apiKey": "sk-abcdef123456"},
        "planner": {"api_key": "sk-zzz999888", "model": "deepseek-v4-flash"},
    }
    safe = redact(body)
    text = str(safe)
    assert "abcdef123456" not in text
    assert "zzz999888" not in text
    # 非敏感字段必须保留（否则日志失去排障价值）
    assert safe["jobId"] == 17
    assert safe["provider"]["baseUrl"] == "https://api.example.com/v1"
    assert safe["planner"]["model"] == "deepseek-v4-flash"


def test_redact_catches_bare_key_values_without_key_name():
    # 键名没暴露，但值本身就是密钥 —— 兜底正则必须接住
    assert "sk-abcdef123456" not in redact("Authorization: sk-abcdef123456")


def test_redact_does_not_mutate_input():
    body = {"apiKey": "sk-abcdef123456"}
    safe = redact(body)
    assert body["apiKey"] == "sk-abcdef123456", "不得原地修改调用方对象"
    assert safe["apiKey"] != body["apiKey"]


def test_redact_handles_list_and_deep_nesting():
    body = {"items": [{"token": "abcdef123456"}, {"ok": 1}]}
    safe = redact(body)
    assert "abcdef123456" not in str(safe)
    assert safe["items"][1]["ok"] == 1


def test_mask_secret_short_value_is_all_stars():
    # 太短的值打码后不能反而暴露长度之外的信息
    assert mask_secret("abc") == "***"


def test_prompt_digest_hides_text_but_keeps_len():
    digest = prompt_digest("把这张图调暖一点")
    assert "调暖" not in digest
    assert "len=" in digest and "sha=" in digest
