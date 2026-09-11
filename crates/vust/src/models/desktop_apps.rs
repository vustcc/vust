//! 桌面应用模型：桌面快捷方式数据结构与数据库访问。

use crate::state::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, FromRow)]
struct DesktopAppItemRecord {
    pub node_id: String,
    pub app_id: String,
    pub sort_order: i64,
    pub visible: i64,
    pub metadata: Option<String>,
}

/// 桌面应用项的元数据（存储在 metadata JSON 列中）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppMetadata {
    pub x: Option<f64>,
    pub y: Option<f64>,
    /// 套件停用前的桌面可见状态，用于重新启用时恢复用户原有选择。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite_visible_before_disable: Option<bool>,
}

/// 桌面应用项的写入载荷。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppItemPayload {
    pub app_id: String,
    pub sort_order: i64,
    pub visible: bool,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    /// 套件停用前的桌面可见状态，由后端在保留停用套件记录时使用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite_visible_before_disable: Option<bool>,
}

/// 桌面应用配置的批量写入载荷。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppsPayload {
    pub apps: Vec<DesktopAppItemPayload>,
}

/// 桌面应用项的读取模型。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppItem {
    pub node_id: String,
    pub app_id: String,
    pub sort_order: i64,
    pub visible: bool,
    pub x: Option<f64>,
    pub y: Option<f64>,
    /// 套件停用前的桌面可见状态，由后端用于生命周期恢复。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite_visible_before_disable: Option<bool>,
}

/// 读取已保存的桌面应用配置，按排序返回。
pub async fn fetch_desktop_apps(
    pool: &DbPool,
    node_id: &str,
) -> sqlx::Result<Option<Vec<DesktopAppItem>>> {
    let records = sqlx::query_as::<_, DesktopAppItemRecord>(
        r#"
        SELECT node_id, app_id, sort_order, visible, metadata
        FROM desktop_apps
        WHERE node_id = ?1
        ORDER BY sort_order ASC, app_id ASC
        "#,
    )
    .bind(node_id)
    .fetch_all(pool)
    .await?;

    if records.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        records
            .into_iter()
            .map(|record| {
                let (x, y, suite_visible_before_disable) = if let Some(meta_str) = record.metadata {
                    if let Ok(meta) = serde_json::from_str::<DesktopAppMetadata>(&meta_str) {
                        (meta.x, meta.y, meta.suite_visible_before_disable)
                    } else {
                        (None, None, None)
                    }
                } else {
                    (None, None, None)
                };

                DesktopAppItem {
                    node_id: record.node_id,
                    app_id: record.app_id,
                    sort_order: record.sort_order,
                    visible: record.visible != 0,
                    x,
                    y,
                    suite_visible_before_disable,
                }
            })
            .collect(),
    ))
}

/// 用新配置覆盖桌面应用列表并写入数据库。
pub async fn replace_desktop_apps(
    pool: &DbPool,
    node_id: &str,
    items: &[DesktopAppItemPayload],
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        DELETE FROM desktop_apps
        WHERE node_id = ?1
        "#,
    )
    .bind(node_id)
    .execute(&mut *tx)
    .await?;

    for item in items {
        let metadata = DesktopAppMetadata {
            x: item.x,
            y: item.y,
            suite_visible_before_disable: item.suite_visible_before_disable,
        };
        let metadata_str = serde_json::to_string(&metadata).ok();

        sqlx::query(
            r#"
            INSERT INTO desktop_apps (node_id, app_id, sort_order, visible, pinned, metadata)
            VALUES (?, ?, ?, ?, 0, ?)
            "#,
        )
        .bind(node_id)
        .bind(&item.app_id)
        .bind(item.sort_order)
        .bind(if item.visible { 1 } else { 0 })
        .bind(metadata_str)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(())
}

/// 隐藏套件应用的桌面快捷方式，并记录停用前的可见状态。
pub async fn hide_suite_desktop_apps(
    pool: &DbPool,
    node_id: &str,
    app_ids: &[String],
) -> sqlx::Result<()> {
    for app_id in app_ids {
        let existing = sqlx::query_as::<_, DesktopAppItemRecord>(
            r#"
            SELECT node_id, app_id, sort_order, visible, metadata
            FROM desktop_apps
            WHERE node_id = ?1
              AND app_id = ?2
            "#,
        )
        .bind(node_id)
        .bind(app_id)
        .fetch_optional(pool)
        .await?;

        if let Some(record) = existing {
            let mut metadata = parse_metadata(record.metadata.as_deref());
            metadata.suite_visible_before_disable = Some(record.visible != 0);
            let metadata_str = serde_json::to_string(&metadata).ok();
            sqlx::query(
                r#"
                UPDATE desktop_apps
                   SET visible = 0,
                       metadata = ?3
                 WHERE node_id = ?1
                   AND app_id = ?2
                "#,
            )
            .bind(node_id)
            .bind(app_id)
            .bind(metadata_str)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// 删除套件应用的桌面快捷方式状态。
pub async fn delete_desktop_apps(
    pool: &DbPool,
    node_id: &str,
    app_ids: &[String],
) -> sqlx::Result<()> {
    for app_id in app_ids {
        sqlx::query("DELETE FROM desktop_apps WHERE node_id = ?1 AND app_id = ?2")
            .bind(node_id)
            .bind(app_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

fn parse_metadata(metadata: Option<&str>) -> DesktopAppMetadata {
    metadata
        .and_then(|value| serde_json::from_str::<DesktopAppMetadata>(value).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{DesktopAppItemPayload, fetch_desktop_apps, replace_desktop_apps};
    use crate::test_support::setup_test_db;

    #[tokio::test]
    async fn fetch_returns_none_when_empty() {
        let pool = setup_test_db().await;
        let items = fetch_desktop_apps(&pool, "local").await.unwrap();
        assert!(items.is_none());
    }

    #[tokio::test]
    async fn replace_desktop_apps_roundtrip() {
        let pool = setup_test_db().await;
        let payload = vec![
            DesktopAppItemPayload {
                app_id: "docker-manager".to_string(),
                sort_order: 2,
                visible: true,
                x: Some(100.0),
                y: Some(200.0),
                suite_visible_before_disable: None,
            },
            DesktopAppItemPayload {
                app_id: "file-editor".to_string(),
                sort_order: 1,
                visible: false,
                x: None,
                y: None,
                suite_visible_before_disable: None,
            },
        ];
        replace_desktop_apps(&pool, "local", &payload)
            .await
            .unwrap();

        let items = fetch_desktop_apps(&pool, "local").await.unwrap().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].app_id, "file-editor");
        assert_eq!(items[0].sort_order, 1);
        assert!(!items[0].visible);
        assert_eq!(items[0].x, None);
        assert_eq!(items[1].app_id, "docker-manager");
        assert_eq!(items[1].sort_order, 2);
        assert!(items[1].visible);
        assert_eq!(items[1].x, Some(100.0));
        assert_eq!(items[1].y, Some(200.0));
    }
}
