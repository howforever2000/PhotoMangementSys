"""作业开跑前的 provider 连通性预检（BUG-2026-1122-007）

一个作业只探一次「这个工具真正会用到的能力位」：端口不通就立刻以可操作的文案
失败，而不是拉着一批照片逐张撞同一个网络错误（单张还要重试 3 次），最后每张
失败都被折叠成一句泛化的「上传图片过大」。

口径全部派生，不新增手写名单：
  - 用哪些能力位 ← `catalog.provider_kind_groups`（`providerPolicy` 唯一真相源，
    与前端 `utils/modelOrigin.ts` 同源）；
  - `upscale|edit` 的回落 ← 组内取第一个已配的（与设置界面「未配置时回落图像编辑」
    同一语义），组内两个都没配就跳过，交给工作流原来的「未配置 provider」报错。

`timeout` 与 `refused/reset` 区别对待：超时往往是本地网关正在加载模型（服务确实
活着），预检让步继续；「没人监听 / 被掐断」不会自愈，直接失败。

本模块只做「取值 → 探测 → 抛错」，不含图像处理，也不含重试编排。
"""
from __future__ import annotations

from .. import catalog
from ..logutil import elog, log
from .neterr import endpoint_of, kind_of
from .openai_compat import ProviderError, probe_reachable

# 预检单次上限：只是 `/models` 这一跳，5s 足够；本地网关加载慢时按 timeout 让步继续，
# 不会比真实出图（分钟级）更早判死。
PREFLIGHT_TIMEOUT = 5.0


def _bound(providers, kind: str):
    cfg = getattr(providers, kind, None)
    if cfg is None or not (cfg.baseUrl or "").strip():
        return None
    return cfg


def targets_for(providers, tool_id: str) -> list[tuple[str, object]]:
    """这次作业真的要探的 (能力位, provider) 列表：每组取第一个已配的，并按地址去重。

    去重是因为同一家供应商常被多个能力位复用（如 vision / edit 同一个 baseUrl），
    没必要对同一地址连探两次。
    """
    out: list[tuple[str, object]] = []
    seen: set[tuple[str, str]] = set()
    for group in catalog.provider_kind_groups(tool_id):
        for kind in group:
            cfg = _bound(providers, kind)
            if cfg is None:
                continue
            key = (cfg.baseUrl.strip(), cfg.apiKey or "")
            if key not in seen:
                seen.add(key)
                out.append((kind, cfg))
            break
    return out


def ensure_reachable(providers, tool_id: str) -> None:
    """预检工具所需的 provider；不可达抛 `ProviderError`，作业随即失败（不进逐张循环）。"""
    for kind, cfg in targets_for(providers, tool_id):
        ep = endpoint_of(cfg.baseUrl)
        try:
            status = probe_reachable(cfg.baseUrl, cfg.apiKey, timeout=PREFLIGHT_TIMEOUT)
        except ProviderError as e:
            if kind_of(e.__cause__) == "timeout":
                # 端口有服务但暂时没应答（本地网关在加载模型）：让真实请求去定论
                elog(f"PREFLIGHT 超时，继续尝试 tool={tool_id} kind={kind} {ep} | {e}")
                continue
            elog(f"PREFLIGHT 失败 tool={tool_id} kind={kind} {ep} | {e}")
            raise ProviderError(f"「{kind}」能力位预检失败 —— {e}") from e
        log(f"PREFLIGHT ok tool={tool_id} kind={kind} {ep} http={status}")
