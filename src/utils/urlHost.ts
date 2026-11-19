/**
 * baseUrl → 主机名 / 是否本机回环（FEAT-199）
 *
 * 为什么单独一个文件：这两条判定本来埋在 `AiSettingsDialog.vue` 的私有函数里
 * （只为了给供应商起个默认显示名）。FEAT-199 要判「这家供应商是不是本机
 * Ollama / LM Studio」，同一份口径现在有两个消费者（设置弹窗 + 模型来源标注），
 * 各写一份必然漂移 —— 与 `modelCatalog.ts` 把分区判定收出来是同一个理由。
 *
 * 只做字符串处理：不发请求、不校验可达性（「本机地址」≠「本机服务起着」）。
 */

/**
 * 从 baseUrl 取 host（不含端口，与 Rust `ai_settings.rs::host_of` 同口径）。
 *
 * 兼容三种写法：带 scheme / 不带 scheme / IPv6 方括号（`http://[::1]:8000/v1`）。
 * 取不到时返回原串，不让显示名变成空 —— 空名字比难看的名字更难排查。
 */
export function hostOf(baseUrl: string): string {
  const s = baseUrl.trim();
  if (!s) return "";
  const afterScheme = s.includes("://") ? s.slice(s.indexOf("://") + 3) : s;
  const hostport = afterScheme.split("/")[0] ?? afterScheme;
  if (hostport.startsWith("[")) {
    const end = hostport.indexOf("]");
    return end > 0 ? hostport.slice(1, end) : hostport;
  }
  const colon = hostport.indexOf(":");
  return colon >= 0 ? hostport.slice(0, colon) : hostport;
}

/**
 * 该 host 是不是本机回环。
 *
 * 判据：`127.0.0.0/8`、IPv6 `::1`、`localhost`（含 `*.localhost`）、`0.0.0.0`。
 * 注意 `0.0.0.0` 只在**监听侧**表示任意网卡，作为客户端目标时等于本机。
 * `host.docker.internal` 一类不算 —— 它不是回环地址，照片仍会经网络栈出去。
 */
export function isLoopbackHost(host: string): boolean {
  const h = host.trim().toLowerCase().replace(/^\[|\]$/g, "");
  if (!h) return false;
  if (h === "localhost" || h.endsWith(".localhost")) return true;
  if (h === "::1" || h === "0.0.0.0") return true;
  return /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(h);
}

/** 便捷合并：baseUrl 直接判回环（空串一律 false） */
export function isLoopbackUrl(baseUrl: string): boolean {
  return isLoopbackHost(hostOf(baseUrl));
}
