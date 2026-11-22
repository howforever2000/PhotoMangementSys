"""provider 网络失败文案与作业前预检（BUG-2026-1122-007）。

钉死的语义：
  1. 「端口没人监听（10061）」与「连上后被掐断（10054）」必须给出不同文案 ——
     前者让人去启动服务，后者才让人去压分辨率；两者混用会把排查引到死路上。
  2. 能力位口径来自 catalog 的 `providerPolicy`：`upscale|edit` 在超分未配置时
     回落编辑位（与设置界面一致），同一地址不重复探测。
  3. 预检只拦「不可自愈」的失败；超时（本地网关正在加载）让步，交给真实请求定论。
"""
import os
import sys
import unittest
from unittest import mock

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import catalog  # noqa: E402
from ai_studio.providers import neterr, preflight  # noqa: E402
from ai_studio.providers.openai_compat import ProviderError  # noqa: E402
from ai_studio.schemas import ProviderConfig, Providers  # noqa: E402

REFUSED = (
    "HTTPConnectionPool(host='127.0.0.1', port=8000): Max retries exceeded with url: "
    "/v1/images/generations (Caused by NewConnectionError(\"HTTPConnection(host='127.0.0.1', "
    "port=8000): Failed to establish a new connection: [WinError 10061] 由于目标计算机积极拒绝，无法连接。\"))"
)
RESET = (
    "HTTPSConnectionPool(host='gw.example.com', port=443): Max retries exceeded with url: "
    "/v1/images/generations (Caused by ConnectionResetError(10054, '远程主机强迫关闭了一个现有的连接。'))"
)
TIMEOUT = "HTTPSConnectionPool(host='gw.example.com', port=443): Read timed out. (read timeout=5)"


class TestEndpoint(unittest.TestCase):
    def test_keeps_host_and_port(self):
        self.assertEqual(neterr.endpoint_of("http://127.0.0.1:8000/v1"), "127.0.0.1:8000")

    def test_defaults_port_by_scheme(self):
        self.assertEqual(neterr.endpoint_of("https://dashscope.aliyuncs.com/x"), "dashscope.aliyuncs.com:443")
        self.assertEqual(neterr.endpoint_of("http://gw.local"), "gw.local:80")

    def test_never_returns_empty(self):
        self.assertTrue(neterr.endpoint_of(""))
        self.assertTrue(neterr.endpoint_of("   "))


class TestDescribe(unittest.TestCase):
    def test_refused_points_at_service(self):
        msg = neterr.describe("http://127.0.0.1:8000/v1", OSError(REFUSED))
        self.assertIn("127.0.0.1:8000", msg)
        self.assertIn("没有服务在监听", msg)
        self.assertNotIn("上传体过大", msg)

    def test_reset_points_at_size(self):
        msg = neterr.describe("https://gw.example.com/v1", OSError(RESET))
        self.assertIn("连接被中途重置", msg)
        self.assertIn("减小输入分辨率", msg)

    def test_timeout(self):
        self.assertIn("超时", neterr.describe("https://gw.example.com/v1", OSError(TIMEOUT)))

    def test_dns(self):
        msg = neterr.describe("https://nope.invalid/v1", OSError("getaddrinfo failed: Name or service not known"))
        self.assertIn("域名解析失败", msg)

    def test_kind_of_none_is_other(self):
        self.assertEqual(neterr.kind_of(None), "other")


class TestKindGroups(unittest.TestCase):
    def test_single_edit(self):
        self.assertEqual(catalog.provider_kind_groups("beauty"), [["edit"]])

    def test_plus_means_both(self):
        self.assertEqual(catalog.provider_kind_groups("smart_edit"), [["vision"], ["edit"]])

    def test_none_policy(self):
        self.assertEqual(catalog.provider_kind_groups("upscale"), [])
        self.assertEqual(catalog.provider_kind_groups("no-such-tool"), [])


def _providers(edit=None, vision=None, upscale=None):
    return Providers(
        edit=ProviderConfig(baseUrl=edit, model="m") if edit else None,
        vision=ProviderConfig(baseUrl=vision, model="m") if vision else None,
        upscale=ProviderConfig(baseUrl=upscale, model="m") if upscale else None,
    )


class TestTargets(unittest.TestCase):
    def test_prefers_upscale_then_falls_back_to_edit(self):
        with mock.patch.object(catalog, "provider_kind_groups", return_value=[["upscale", "edit"]]):
            hit = preflight.targets_for(_providers(edit="http://127.0.0.1:8000/v1"), "any")
            self.assertEqual([k for k, _ in hit], ["edit"])
            hit = preflight.targets_for(
                _providers(edit="http://127.0.0.1:8000/v1", upscale="http://127.0.0.1:9000/v1"), "any"
            )
            self.assertEqual([k for k, _ in hit], ["upscale"])

    def test_same_address_probed_once(self):
        with mock.patch.object(catalog, "provider_kind_groups", return_value=[["vision"], ["edit"]]):
            hit = preflight.targets_for(
                _providers(vision="http://127.0.0.1:8000/v1", edit="http://127.0.0.1:8000/v1"), "any"
            )
            self.assertEqual(len(hit), 1)

    def test_unconfigured_group_is_skipped(self):
        self.assertEqual(preflight.targets_for(_providers(), "beauty"), [])


class TestEnsureReachable(unittest.TestCase):
    def _raise(self, exc):
        def fake(base, api_key, timeout=5.0):
            raise ProviderError(neterr.describe(base, exc)) from exc

        return fake

    def test_refused_fails_fast(self):
        with mock.patch.object(preflight, "probe_reachable", self._raise(OSError(REFUSED))):
            with self.assertRaises(ProviderError) as ctx:
                preflight.ensure_reachable(_providers(edit="http://127.0.0.1:8000/v1"), "beauty")
        self.assertIn("预检失败", str(ctx.exception))
        self.assertIn("没有服务在监听", str(ctx.exception))

    def test_timeout_is_tolerated(self):
        with mock.patch.object(preflight, "probe_reachable", self._raise(OSError(TIMEOUT))):
            preflight.ensure_reachable(_providers(edit="http://127.0.0.1:8000/v1"), "beauty")


if __name__ == "__main__":
    unittest.main()
