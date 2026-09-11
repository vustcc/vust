//! Agent 操作审计 outbox：持久排队、批量读取、重试与确认。

use chrono::Utc;
use sqlx::FromRow;
use vust_contracts::logging::AgentOperationEvent;

use crate::state::DbPool;

/// 待投递批次项。
#[derive(Debug, FromRow)]
struct OutboxRow {
    event_id: String,
    event_json: String,
}

/// 幂等写入本地 outbox。
pub async fn enqueue(pool: &DbPool, event: &AgentOperationEvent) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO operation_event_outbox (event_id,event_json,created_at,next_attempt_at) VALUES (?,?,?,?)")
        .bind(&event.event_id).bind(serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string()))
        .bind(&event.occurred_at).bind(Utc::now().to_rfc3339()).execute(pool).await?;
    Ok(())
}

/// 读取当前允许重试的批次。
pub async fn pending(pool: &DbPool, limit: u32) -> Result<Vec<AgentOperationEvent>, sqlx::Error> {
    let rows = sqlx::query_as::<_, OutboxRow>("SELECT event_id,event_json FROM operation_event_outbox WHERE delivered_at IS NULL AND next_attempt_at <= ? ORDER BY created_at,event_id LIMIT ?")
        .bind(Utc::now().to_rfc3339()).bind(i64::from(limit.min(100))).fetch_all(pool).await?;
    Ok(rows.into_iter().filter_map(|row| serde_json::from_str(&row.event_json).map_err(|error| tracing::error!(event_id=%row.event_id,%error,"Operation audit outbox record is corrupted")).ok()).collect())
}

/// 确认 Master 已接受的事件。
pub async fn acknowledge(pool: &DbPool, event_ids: &[String]) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    for event_id in event_ids {
        sqlx::query("UPDATE operation_event_outbox SET delivered_at=? WHERE event_id=?")
            .bind(&now)
            .bind(event_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// 记录批次失败并按有界指数退避安排下次投递。
pub async fn mark_failed(pool: &DbPool, event_ids: &[String]) -> Result<(), sqlx::Error> {
    for event_id in event_ids {
        sqlx::query("UPDATE operation_event_outbox SET attempts=attempts+1,next_attempt_at=strftime('%Y-%m-%dT%H:%M:%fZ','now','+' || min(300, 2 << min(attempts,7)) || ' seconds') WHERE event_id=?")
            .bind(event_id).execute(pool).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::setup_test_db;
    use std::collections::BTreeMap;
    use vust_contracts::logging::*;
    #[tokio::test]
    async fn persists_and_acknowledges_event() {
        let pool = setup_test_db().await;
        let event = AgentOperationEvent {
            event_id: "evt".into(),
            occurred_at: Utc::now().to_rfc3339(),
            module: OperationModule::Docker,
            event_code: "docker_started".into(),
            event_label: None,
            actor: OperationActor {
                kind: OperationActorKind::User,
                user_id: Some(7),
                display_name: "admin".into(),
            },
            client_ip: Some("127.0.0.1".into()),
            target: None,
            outcome: OperationOutcome::Success,
            impact: OperationImpact::Info,
            trace_id: "trace".into(),
            task_id: None,
            parameters: BTreeMap::new(),
            error_code: None,
            error_summary: None,
        };
        enqueue(&pool, &event).await.unwrap();
        enqueue(&pool, &event).await.unwrap();
        let pending_events = pending(&pool, 20).await.unwrap();
        assert_eq!(pending_events.len(), 1);
        assert_eq!(pending_events[0].actor.user_id, Some(7));
        acknowledge(&pool, &["evt".into()]).await.unwrap();
        assert!(pending(&pool, 20).await.unwrap().is_empty());
    }
}
