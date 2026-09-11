//! 应用管理 API：应用列表与元数据维护接口。

use crate::models::apps::list_apps;
use crate::state::AppState;
use crate::types::{ApiResponse, ApiResult};
use axum::http::{HeaderMap, header};
use axum::{
    Router,
    extract::{Query, State},
    response::IntoResponse,
    routing::get,
};
use serde::Deserialize;
use std::sync::Arc;

/// 应用目录查询参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppListQuery {
    pub node_id: Option<String>,
}

/// 构建应用目录相关的路由集合。
pub fn apps_router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(list))
}

/// 返回应用目录列表。
pub async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AppListQuery>,
) -> ApiResult<impl IntoResponse> {
    let locale = resolve_request_locale(&headers);
    let node_id = query.node_id.as_deref().unwrap_or("local");
    let apps = list_apps(&state.metadata_db, locale.as_deref(), Some(node_id)).await?;
    Ok(ApiResponse::success_with_raw("Applications loaded", apps).into_response())
}

fn resolve_request_locale(headers: &HeaderMap) -> Option<String> {
    let raw = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())?;
    // 应用库只需要当前首选语言，用于解析套件应用入口标题。
    raw.split(',')
        .filter_map(|part| part.trim().split(';').next())
        .map(str::trim)
        .find(|part| !part.is_empty())
        .map(|part| part.replace('_', "-"))
}
