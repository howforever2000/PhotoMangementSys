//! 成本台账：`agent_runs` 增量落库。
//!
//! 为什么在 Rust 而不是 Python（docs/v5.md §1、§9）：钱与审计要落在**不会被 kill
//! 的一侧**。Python 半路死掉时，已经轮询到的成本仍然在库里。
//!
//! 增量做法：每 400ms 轮询一次就把"见到的新步骤"写进去，
//! `UNIQUE(user_id, job_id, step_idx)` + `INSERT OR IGNORE` 天然幂等 ——
//! 不需要在内存里记"哪些写过了"，进程重启也不会重复记账。
//!
//! 本版本成本恒为 0（本地算法），但管道现在就要通：等接上 provider，
//! 只要 `cost` 字段开始有值，台账就已经在正确的位置落行了。

use rusqlite::params;

use super::job::AgentStep;

/// 建表（幂等）。表名/列名一旦上线就是审计口径，改字段等于改账。
pub fn init(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_runs (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id     INTEGER NOT NULL,
            job_id      INTEGER NOT NULL,
            recipe_id   TEXT    NOT NULL,
            step_idx    INTEGER NOT NULL,
            tool        TEXT    NOT NULL,
            model       TEXT,
            cost        REAL    NOT NULL DEFAULT 0,
            estimated   INTEGER NOT NULL DEFAULT 0,
            created_at  INTEGER NOT NULL,
            UNIQUE(user_id, job_id, step_idx)
        );
        CREATE INDEX IF NOT EXISTS idx_agent_runs_job ON agent_runs(user_id, job_id);",
    )
    .map_err(|e| format!("建 agent_runs 表失败: {e}"))?;
    Ok(())
}

/// 记录本轮见到的新步骤，返回新增行数（幂等：重复调用不会重复记）。
pub fn record(
    conn: &rusqlite::Connection,
    user_id: i64,
    job_id: i64,
    recipe_id: &str,
    steps: &[AgentStep],
    now: i64,
) -> Result<usize, String> {
    let mut inserted = 0usize;
    let mut stmt = conn
        .prepare(
            "INSERT OR IGNORE INTO agent_runs
                (user_id, job_id, recipe_id, step_idx, tool, model, cost, estimated, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .map_err(|e| format!("准备 agent_runs 写入失败: {e}"))?;

    for s in steps {
        // 有 model 才可能花钱；纯本地步骤 cost 恒 0，不需要标 estimated
        let estimated = if s.model.is_some() && s.cost <= 0.0 { 1 } else { 0 };
        let n = stmt
            .execute(params![
                user_id,
                job_id,
                recipe_id,
                s.idx,
                s.tool,
                s.model,
                s.cost,
                estimated,
                now
            ])
            .map_err(|e| format!("写 agent_runs 失败: {e}"))?;
        inserted += n;
    }
    Ok(inserted)
}

/// 某个 job 已记账的总额（UI 可以拿它做"已花 ≠ 估算"的对照）
#[allow(dead_code)] // 预算面板接进来后使用
pub fn total_cost(conn: &rusqlite::Connection, user_id: i64, job_id: i64) -> Result<f64, String> {
    conn.query_row(
        "SELECT COALESCE(SUM(cost), 0) FROM agent_runs WHERE user_id = ?1 AND job_id = ?2",
        params![user_id, job_id],
        |r| r.get::<_, f64>(0),
    )
    .map_err(|e| format!("读 agent_runs 合计失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> rusqlite::Connection {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        init(&c).unwrap();
        c
    }

    fn step(idx: i64, tool: &str, model: Option<&str>, cost: f64) -> AgentStep {
        AgentStep {
            idx,
            tool: tool.to_string(),
            model: model.map(|m| m.to_string()),
            cost,
            ..Default::default()
        }
    }

    #[test]
    fn record_is_idempotent() {
        let c = conn();
        let steps = vec![step(1, "select", None, 0.0), step(2, "compress", None, 0.0)];
        assert_eq!(record(&c, 1, 17, "compress.target_kb", &steps, 100).unwrap(), 2);
        // 再写一遍（模拟下一次轮询看到同样的步骤）
        assert_eq!(record(&c, 1, 17, "compress.target_kb", &steps, 200).unwrap(), 0);
        assert_eq!(total_cost(&c, 1, 17).unwrap(), 0.0);
    }

    #[test]
    fn incremental_steps_only_add_the_new_ones() {
        let c = conn();
        record(&c, 1, 18, "r", &[step(1, "select", None, 0.0)], 1).unwrap();
        // 下一次轮询多了一步
        let n = record(
            &c,
            1,
            18,
            "r",
            &[step(1, "select", None, 0.0), step(2, "style_transfer", None, 0.0)],
            2,
        )
        .unwrap();
        assert_eq!(n, 1, "只应写入新增的第 2 步");
    }

    #[test]
    fn cost_sums_and_model_steps_mark_estimated_when_cost_unknown() {
        let c = conn();
        let steps = vec![
            step(1, "plan", Some("deepseek-v4-flash"), 0.0), // 有模型但成本未知 → estimated
            step(2, "render", Some("seedream"), 0.15),
        ];
        record(&c, 7, 19, "style.ai_reillustrate", &steps, 10).unwrap();
        assert!((total_cost(&c, 7, 19).unwrap() - 0.15).abs() < 1e-9);

        let est: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM agent_runs WHERE job_id = 19 AND estimated = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(est, 1);
    }

    #[test]
    fn jobs_are_isolated_per_user() {
        let c = conn();
        record(&c, 1, 20, "r", &[step(1, "style_transfer", None, 0.0)], 1).unwrap();
        record(&c, 2, 20, "r", &[step(1, "style_transfer", None, 0.0)], 1).unwrap();
        // 同一 job_id 不同用户各有一行（唯一键含 user_id）
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM agent_runs WHERE job_id = 20", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }
}
