//! 调 Python 侧车的薄客户端（回环 + `X-Agent-Token`）。
//!
//! 遵循 README「客户端保持薄壳」：这里只做 HTTP 与错误分类，不做业务判断。
//! 三个端点之外不要加东西 —— 增加端点等于增加 Python 的能力面，
//! 而 Python 侧刻意没有"能任意查库"的工具（docs/v5.md §7）。

use serde_json::Value;

/// 一次调用的超时。轮询很轻，给 3s；submit 要装配候选池，给 10s。
const T_STATUS_MS: u64 = 3_000;
const T_SUBMIT_MS: u64 = 10_000;
const T_CANCEL_MS: u64 = 3_000;
/// 探针：内部可能要先解析 DNS、再走一次对话 ping，deep 还会真出一张图
const T_PROBE_MS: u64 = 90_000;

/// 状态查询的失败分类。
///
/// `NotFound` 单独拎出来，是因为它有个**特定的业务含义**：
/// Python 侧重启过 → job 蒸发。它与"网络不通"要给出完全不同的用户文案。
#[derive(Debug)]
pub enum StatusErr {
    /// 404：服务活着，但没有这个 job（服务重启过 / 从未受理）
    NotFound,
    /// 401/503：token 不对或未注入（契约破坏，属内部 bug）
    Unauthorized(u16),
    /// 连不上 / 超时
    Net(String),
    /// 返回体不是预期 JSON
    Parse(String),
}

impl std::fmt::Display for StatusErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatusErr::NotFound => write!(f, "job 不存在（Agent 服务可能已重启）"),
            StatusErr::Unauthorized(c) => write!(f, "Agent 服务拒绝请求（HTTP {c}，token 校验失败）"),
            StatusErr::Net(e) => write!(f, "连不上 Agent 服务：{e}"),
            StatusErr::Parse(e) => write!(f, "Agent 返回体无法解析：{e}"),
        }
    }
}

fn client(ms: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(ms))
        .build()
        .map_err(|e| format!("构造 HTTP 客户端失败: {e}"))
}

/// 提交 job，返回 Python 侧确认的 jobId。
pub async fn submit(base: &str, token: &str, body: &Value) -> Result<i64, String> {
    let resp = client(T_SUBMIT_MS)?
        .post(format!("{base}/agent/submit"))
        .header("X-Agent-Token", token)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("提交 Agent 任务失败（连不上服务）：{e}"))?;

    let code = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !code.is_success() {
        // 409 = 单飞冲突；400 = 契约不符；401 = token 不对。原样带出来便于定位
        return Err(format!(
            "提交 Agent 任务被拒绝（HTTP {}）：{}",
            code.as_u16(),
            truncate(&text, 300)
        ));
    }
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| format!("提交响应无法解析：{e}；原文：{}", truncate(&text, 200)))?;
    v.get("jobId")
        .and_then(|x| x.as_i64())
        .ok_or_else(|| format!("提交响应缺少 jobId：{}", truncate(&text, 200)))
}

/// 查状态。失败被分成四类（见 `StatusErr`）。
pub async fn status(base: &str, token: &str, job_id: i64) -> Result<Value, StatusErr> {
    let resp = client(T_STATUS_MS)
        .map_err(StatusErr::Net)?
        .get(format!("{base}/agent/jobs/{job_id}"))
        .header("X-Agent-Token", token)
        .send()
        .await
        .map_err(|e| StatusErr::Net(e.to_string()))?;

    let code = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    match code {
        200 => serde_json::from_str(&text)
            .map_err(|e| StatusErr::Parse(format!("{e}；原文：{}", truncate(&text, 200)))),
        404 => Err(StatusErr::NotFound),
        401 | 403 | 503 => Err(StatusErr::Unauthorized(code)),
        _ => Err(StatusErr::Parse(format!(
            "HTTP {code}：{}",
            truncate(&text, 200)
        ))),
    }
}

/// 请求取消。失败不致命（轮询循环另有取消标记双保险），所以只返回错误文案。
pub async fn cancel(base: &str, token: &str, job_id: i64) -> Result<(), String> {    let resp = client(T_CANCEL_MS)?
        .post(format!("{base}/agent/jobs/{job_id}/cancel"))
        .header("X-Agent-Token", token)
        .send()
        .await
        .map_err(|e| format!("请求取消失败：{e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("请求取消被拒绝（HTTP {}）", resp.status().as_u16()))
    }
}

/// 端点探针（`POST /agent/probe`）。返回原始报告（camelCase），前端直接渲染。
pub async fn probe(base: &str, token: &str, body: &Value) -> Result<Value, String> {
    let resp = client(T_PROBE_MS)?
        .post(format!("{base}/agent/probe"))
        .header("X-Agent-Token", token)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("探测失败（连不上 Agent 服务）：{e}"))?;
    let code = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !code.is_success() {
        return Err(format!(
            "探测被拒绝（HTTP {}）：{}",
            code.as_u16(),
            truncate(&text, 300)
        ));
    }
    serde_json::from_str(&text)
        .map_err(|e| format!("探测报告无法解析：{e}；原文：{}", truncate(&text, 200)))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_untouched() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("abcdef", 3), "abc…");
    }

    #[test]
    fn status_err_messages_are_actionable() {
        // 三类文案必须能指导用户下一步动作，不能只说"出错了"
        assert!(StatusErr::NotFound.to_string().contains("重启"));
        assert!(StatusErr::Unauthorized(401).to_string().contains("token"));
        assert!(StatusErr::Net("refused".into()).to_string().contains("连不上"));
    }

    #[tokio::test]
    async fn status_on_dead_port_is_net_error() {
        let e = status("http://127.0.0.1:1", "t", 1).await.unwrap_err();
        assert!(matches!(e, StatusErr::Net(_)), "实得 {e:?}");
    }
}
