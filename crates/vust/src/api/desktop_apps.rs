//! 桌面应用 API：桌面快捷方式与排序相关接口。

use crate::models::apps::list_apps;
use crate::models::desktop_apps::{
    DesktopAppItem, DesktopAppItemPayload, fetch_desktop_apps, replace_desktop_apps,
};
use crate::state::AppState;
use crate::types::{ApiResponse, ApiResult};
use axum::{Json, Router, extract::Query, extract::State, response::IntoResponse, routing::get};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// 封装桌面应用列表与是否已配置过的标记。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppsResponse {
    pub apps: Vec<DesktopAppItem>,
    pub has_record: bool,
}

/// 桌面快捷方式保存范围。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppsQuery {
    pub node_id: Option<String>,
}

/// 桌面快捷方式保存请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppsUpdateRequest {
    pub node_id: Option<String>,
    pub apps: Vec<DesktopAppItemPayload>,
}

/// 构建桌面应用相关的路由集合。
pub fn desktop_apps_router() -> Router<Arc<AppState>> {
    Router::new().route("/shortcuts", get(list).put(update))
}

/// 返回当前桌面应用排序与可见性配置。
pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DesktopAppsQuery>,
) -> ApiResult<impl IntoResponse> {
    let node_id = query.node_id.as_deref().unwrap_or("local");
    let app_ids = fetch_desktop_apps(&state.metadata_db, node_id).await?;
    let response = DesktopAppsResponse {
        apps: app_ids.clone().unwrap_or_default(),
        has_record: app_ids.is_some(),
    };
    Ok(ApiResponse::success_with_raw("Desktop applications loaded", response).into_response())
}

/// 保存桌面应用排序与可见性，并补全缺失项的默认值。
pub async fn update(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<DesktopAppsUpdateRequest>,
) -> ApiResult<impl IntoResponse> {
    let node_id = payload.node_id.as_deref().unwrap_or("local");
    let mut seen = HashSet::new();
    let payload_items: Vec<DesktopAppItemPayload> = payload
        .apps
        .into_iter()
        .filter(|item| seen.insert(item.app_id.clone()))
        .collect();
    let catalog = list_apps(&state.metadata_db, None, Some(node_id)).await?;
    let payload_map: HashMap<String, DesktopAppItemPayload> = payload_items
        .into_iter()
        .map(|item| (item.app_id.clone(), item))
        .collect();
    let items: Vec<DesktopAppItemPayload> = catalog
        .into_iter()
        .map(|app| {
            payload_map
                .get(&app.app_id)
                .cloned()
                .unwrap_or(DesktopAppItemPayload {
                    app_id: app.app_id,
                    sort_order: app.sort_order,
                    visible: false,
                    x: None,
                    y: None,
                    suite_visible_before_disable: None,
                })
        })
        .collect();
    replace_desktop_apps(&state.metadata_db, node_id, &items).await?;
    Ok(ApiResponse::ok("Desktop applications saved").into_response())
}
