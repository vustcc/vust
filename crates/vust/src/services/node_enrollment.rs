//! 节点纳管服务：生成 enrollment token 并写入 `node_enrollments`。

use crate::models::node_enrollments::{
    NodeEnrollmentRecord, insert_node_enrollment, revoke_issued_enrollment,
};
use crate::state::DbPool;
use crate::types::new_uuid_v7;
use chrono::{Duration, Utc};
use ring::digest;
use sqlx::{Executor, Sqlite};

/// enrollment token 结果，包含明文 token 与记录 ID。
#[derive(Debug, Clone)]
pub struct EnrollmentTokenIssue {
    pub enrollment_id: String,
    pub token: String,
}

fn token_hash(token: &str) -> String {
    let digest = digest::digest(&digest::SHA256, token.as_bytes());
    hex::encode(digest.as_ref())
}

/// 为节点签发新的 enrollment token 并写入数据库。
pub async fn issue_enrollment_token<'e, E>(
    executor: E,
    node_id: &str,
) -> sqlx::Result<EnrollmentTokenIssue>
where
    E: Executor<'e, Database = Sqlite>,
{
    let enrollment_id = new_uuid_v7();
    let token = format!("ldt_{}", new_uuid_v7().replace('-', ""));
    let now = Utc::now();
    insert_node_enrollment(
        executor,
        &NodeEnrollmentRecord {
            enrollment_id: enrollment_id.clone(),
            node_id: node_id.to_string(),
            token_hash: token_hash(&token),
            token_status: "issued".to_string(),
            expires_at: (now + Duration::hours(24)).to_rfc3339(),
            first_used_at: None,
            last_used_at: None,
            revoked_at: None,
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
        },
    )
    .await?;

    Ok(EnrollmentTokenIssue {
        enrollment_id,
        token,
    })
}

/// 部署失败后撤销本次签发且尚未使用的纳管令牌。
pub async fn revoke_failed_enrollment(pool: &DbPool, enrollment_id: &str) -> sqlx::Result<bool> {
    revoke_issued_enrollment(pool, enrollment_id).await
}

#[cfg(test)]
mod tests {
    use super::{issue_enrollment_token, revoke_failed_enrollment};
    use crate::models::node_enrollments::get_enrollment_by_token_hash;
    use crate::models::nodes::{NewNodeRecord, insert_node};
    use crate::test_support::setup_test_db;
    use chrono::{DateTime, Utc};
    use ring::digest;
    use uuid::Uuid;

    fn token_hash(token: &str) -> String {
        let digest = digest::digest(&digest::SHA256, token.as_bytes());
        hex::encode(digest.as_ref())
    }

    async fn insert_test_node(pool: &crate::state::DbPool, node_id: &str) {
        insert_node(
            pool,
            &NewNodeRecord {
                node_id: node_id.to_string(),
                tenant_id: None,
                name: format!("node-{node_id}"),
                normalized_name: format!("node-{node_id}"),
                group_name: "default".to_string(),
                labels: "[]".to_string(),
                description: None,
                desired_role: None,
                schedulable: true,
                metadata: "{}".to_string(),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn issue_enrollment_token_persists_hashed_token_with_expected_defaults() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        insert_test_node(&pool, &node_id).await;

        let issued = issue_enrollment_token(&pool, &node_id).await.unwrap();
        assert!(issued.token.starts_with("ldt_"));

        let stored = get_enrollment_by_token_hash(&pool, &token_hash(&issued.token))
            .await
            .unwrap()
            .expect("issued enrollment should be queryable by token hash");
        assert_eq!(stored.enrollment_id, issued.enrollment_id);
        assert_eq!(stored.node_id, node_id);
        assert_eq!(stored.token_status, "issued");
        assert!(stored.first_used_at.is_none());
        assert!(stored.last_used_at.is_none());
        assert!(stored.revoked_at.is_none());

        let expires_at = DateTime::parse_from_rfc3339(&stored.expires_at)
            .unwrap()
            .with_timezone(&Utc);
        let delta_hours = (expires_at - Utc::now()).num_hours();
        assert!(
            (23..=24).contains(&delta_hours),
            "enrollment expiry should be about 24h, got {delta_hours}"
        );
    }

    #[tokio::test]
    async fn revoke_failed_enrollment_only_revokes_issued_token() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        insert_test_node(&pool, &node_id).await;

        let issued = issue_enrollment_token(&pool, &node_id).await.unwrap();
        assert!(
            revoke_failed_enrollment(&pool, &issued.enrollment_id)
                .await
                .unwrap()
        );
        assert!(
            !revoke_failed_enrollment(&pool, &issued.enrollment_id)
                .await
                .unwrap()
        );

        let stored = get_enrollment_by_token_hash(&pool, &token_hash(&issued.token))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.token_status, "revoked");
        assert!(stored.revoked_at.is_some());
    }
}
