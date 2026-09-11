//! 应用模型：应用库数据结构与数据库访问。

use crate::models::suites::SuiteManifest;
use crate::state::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, FromRow)]
struct AppRecord {
    pub app_id: String,
    pub i18n_title_key: String,
    pub icon: String,
    pub default_width: i64,
    pub default_height: i64,
    pub min_width: i64,
    pub min_height: i64,
    pub hidden: i64,
    pub sort_order: i64,
}

#[derive(Debug, FromRow)]
struct SuiteAppRecord {
    pub app_id: String,
    pub suite_instance_id: String,
    pub node_id: String,
    pub app_entry_id: String,
    pub title: String,
    pub icon: String,
    pub entry_type: String,
    pub entry_target: String,
    pub default_width: i64,
    pub default_height: i64,
    pub min_width: i64,
    pub min_height: i64,
    pub sort_order: i64,
    pub manifest_json: String,
}

/// 应用目录的精简展示模型。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppSummary {
    pub app_id: String,
    pub i18n_title_key: String,
    pub title: Option<String>,
    /// VDL 图标名称，默认与应用 ID 一致。
    pub icon: String,
    pub default_width: i64,
    pub default_height: i64,
    pub min_width: i64,
    pub min_height: i64,
    pub hidden: bool,
    pub sort_order: i64,
    pub source_type: String,
    pub suite_instance_id: Option<String>,
    pub node_id: Option<String>,
    pub app_entry_id: Option<String>,
    pub entry_type: Option<String>,
    pub entry_target: Option<String>,
}

/// 按排序读取应用目录列表。
pub async fn list_apps(
    pool: &DbPool,
    locale: Option<&str>,
    node_id: Option<&str>,
) -> sqlx::Result<Vec<AppSummary>> {
    let records = sqlx::query_as::<_, AppRecord>(
        r#"
        SELECT app_id,
               i18n_title_key,
               icon,
               default_width,
               default_height,
               min_width,
               min_height,
               hidden,
               sort_order
        FROM apps
        ORDER BY sort_order ASC, app_id ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut apps: Vec<AppSummary> = records
        .into_iter()
        .map(|record| AppSummary {
            app_id: record.app_id,
            i18n_title_key: record.i18n_title_key,
            title: None,
            icon: version_builtin_icon(&record.icon),
            default_width: record.default_width,
            default_height: record.default_height,
            min_width: record.min_width,
            min_height: record.min_height,
            hidden: record.hidden != 0,
            sort_order: record.sort_order,
            source_type: "builtin".to_string(),
            suite_instance_id: None,
            node_id: None,
            app_entry_id: None,
            entry_type: None,
            entry_target: None,
        })
        .collect();

    let suite_records = if let Some(node_id) = node_id {
        sqlx::query_as::<_, SuiteAppRecord>(
            r#"
            SELECT e.app_id,
                   e.suite_instance_id,
                   e.node_id,
                   e.app_entry_id,
                   e.title,
                   e.icon,
                   e.entry_type,
                   e.entry_target,
                   e.default_width,
                   e.default_height,
                   e.min_width,
                   e.min_height,
                   e.sort_order,
                   c.manifest_json
              FROM suite_app_entries e
              JOIN suite_instances i ON i.instance_id = e.suite_instance_id
              JOIN suite_catalog_items c ON c.suite_id = i.suite_id
             WHERE e.enabled = 1
               AND i.status = 'enabled'
               AND e.node_id = ?1
             ORDER BY e.sort_order ASC, e.app_id ASC
            "#,
        )
        .bind(node_id)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, SuiteAppRecord>(
            r#"
            SELECT e.app_id,
                   e.suite_instance_id,
                   e.node_id,
                   e.app_entry_id,
                   e.title,
                   e.icon,
                   e.entry_type,
                   e.entry_target,
                   e.default_width,
                   e.default_height,
                   e.min_width,
                   e.min_height,
                   e.sort_order,
                   c.manifest_json
              FROM suite_app_entries e
              JOIN suite_instances i ON i.instance_id = e.suite_instance_id
              JOIN suite_catalog_items c ON c.suite_id = i.suite_id
             WHERE e.enabled = 1
               AND i.status = 'enabled'
             ORDER BY e.sort_order ASC, e.app_id ASC
            "#,
        )
        .fetch_all(pool)
        .await?
    };

    let suite_apps = suite_records
        .into_iter()
        .map(|record| {
            let manifest = serde_json::from_str::<SuiteManifest>(&record.manifest_json)
                .map_err(|err| sqlx::Error::Decode(Box::new(err)))?;
            let title = manifest
                .app_entries
                .iter()
                .find(|entry| entry.id == record.app_entry_id)
                .map(|entry| manifest.localized_app_entry_title(entry, locale))
                .unwrap_or_else(|| record.title.clone());
            Ok(AppSummary {
                app_id: record.app_id,
                i18n_title_key: String::new(),
                title: Some(title),
                icon: record.icon,
                default_width: record.default_width,
                default_height: record.default_height,
                min_width: record.min_width,
                min_height: record.min_height,
                hidden: false,
                sort_order: record.sort_order,
                source_type: "suite".to_string(),
                suite_instance_id: Some(record.suite_instance_id),
                node_id: Some(record.node_id),
                app_entry_id: Some(record.app_entry_id),
                entry_type: Some(record.entry_type),
                entry_target: Some(record.entry_target),
            })
        })
        .collect::<sqlx::Result<Vec<_>>>()?;
    apps.extend(suite_apps);

    apps.sort_by(|a, b| {
        a.sort_order
            .cmp(&b.sort_order)
            .then_with(|| a.app_id.cmp(&b.app_id))
    });
    Ok(apps)
}

/// 为内置应用静态图标追加构建版本，允许浏览器长期缓存并在新构建后失效。
fn version_builtin_icon(icon: &str) -> String {
    if !icon.starts_with("/images/apps/") || icon.contains('?') {
        return icon.to_string();
    }
    format!(
        "{icon}?v={}-{}",
        crate::build::PKG_VERSION,
        crate::build::SHORT_COMMIT
    )
}

#[cfg(test)]
mod tests {
    use super::{list_apps, version_builtin_icon};
    use crate::test_support::setup_test_db;

    #[tokio::test]
    async fn list_apps_returns_seeded_rows() {
        let pool = setup_test_db().await;
        let apps = list_apps(&pool, None, None).await.unwrap();
        assert!(!apps.is_empty());
        assert!(apps.iter().any(|app| app.app_id == "docker-manager"));
        assert!(
            apps.windows(2)
                .all(|pair| pair[0].sort_order <= pair[1].sort_order)
        );
        let docker = apps
            .iter()
            .find(|app| app.app_id == "docker-manager")
            .unwrap();
        assert!(
            docker
                .icon
                .starts_with("/images/apps/docker-manager.png?v=")
        );
    }

    #[test]
    fn only_versions_builtin_app_image_urls() {
        assert!(
            version_builtin_icon("/images/apps/example.png")
                .starts_with("/images/apps/example.png?v=")
        );
        assert_eq!(version_builtin_icon("settings"), "settings");
        assert_eq!(
            version_builtin_icon("/api/v1/suites/example/assets/suite-icon.png"),
            "/api/v1/suites/example/assets/suite-icon.png"
        );
    }
}
