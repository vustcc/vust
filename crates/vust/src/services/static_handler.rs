//! 静态资源服务：处理前端静态文件请求与缓存策略。

use axum::{
    Json,
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Redirect, Response},
};
use rust_embed::RustEmbed;
use serde::Serialize;

const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
const CACHE_REVALIDATE: &str = "no-cache";

/// 使用 rust-embed 宏，在编译时将前端静态资源打包进二进制文件。
#[derive(RustEmbed)]
#[folder = "../../frontend/dist"]
struct FrontendAssets;

/// API 404 响应的 JSON 载荷。
#[derive(Serialize)]
pub struct NotFound {
    pub code: u16,
    pub message: String,
    pub details: Option<String>,
}

/// 统一的、智能的 Fallback 处理器。
///
/// 这个处理器负责处理所有未被更精确的路由（如 /api/...）匹配的请求。
/// 它会根据请求的路径来判断：
/// - 如果是未匹配到的 API 请求 (路径以 /api/ 开头)，则返回 JSON 格式的 404 错误。
/// - 如果是其他请求，则视为前端页面或静态资源请求，返回对应的文件或 `index.html`。
pub async fn fallback_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // 关键逻辑：判断请求是否是针对 API 的
    if path.starts_with("api/") {
        return Redirect::to("/404").into_response();
    }

    // --- 如果不是 API 请求，则执行服务前端应用的逻辑 ---
    // 将请求路径转换为嵌入资源中的文件路径
    let mut asset_path = path.to_string();
    if asset_path.is_empty() {
        asset_path = "index.html".to_string();
    }

    match FrontendAssets::get(&asset_path) {
        Some(content) => {
            // 找到了对应的静态文件，直接返回
            let body = Body::from(content.data);
            let mime_type = mime_guess::from_path(&asset_path).first_or_octet_stream();
            Response::builder()
                .header(header::CONTENT_TYPE, mime_type.as_ref())
                .header(header::CACHE_CONTROL, cache_control_for(&uri, &asset_path))
                .body(body)
                .unwrap()
        }
        None => {
            // 如果找不到文件（例如，访问 /dashboard），则返回 index.html
            match FrontendAssets::get("index.html") {
                Some(content) => {
                    let body = Body::from(content.data);
                    let mime_type = mime_guess::from_path("index.html").first_or_octet_stream();
                    Response::builder()
                        .header(header::CONTENT_TYPE, mime_type.as_ref())
                        .header(header::CACHE_CONTROL, CACHE_REVALIDATE)
                        .body(body)
                        .unwrap()
                }
                None => handler_404().await.into_response(),
            }
        }
    }
}

/// 根据资源命名和版本参数选择浏览器缓存策略。
fn cache_control_for(uri: &Uri, asset_path: &str) -> &'static str {
    let is_hashed_build_asset = asset_path.starts_with("assets/");
    let is_versioned_app_icon = asset_path.starts_with("images/apps/")
        && uri.query().is_some_and(|query| {
            query.split('&').any(|pair| {
                let mut parts = pair.splitn(2, '=');
                parts.next() == Some("v") && parts.next().is_some_and(|value| !value.is_empty())
            })
        });

    if is_hashed_build_asset || is_versioned_app_icon {
        CACHE_IMMUTABLE
    } else {
        CACHE_REVALIDATE
    }
}

async fn handler_404() -> (StatusCode, Json<NotFound>) {
    let message = NotFound {
        code: StatusCode::NOT_FOUND.as_u16(),
        message: "The requested API resource was not found.".to_string(),
        details: Some("Please check the API endpoint URL and try again.".to_string()),
    };
    (StatusCode::NOT_FOUND, Json(message))
}

#[cfg(test)]
mod tests {
    use super::{CACHE_IMMUTABLE, CACHE_REVALIDATE, cache_control_for};
    use axum::http::Uri;

    #[test]
    fn versioned_app_icons_are_immutable() {
        let uri = Uri::from_static("/images/apps/docker-manager.png?v=0.1.0-alpha.1-12345678");
        assert_eq!(
            cache_control_for(&uri, "images/apps/docker-manager.png"),
            CACHE_IMMUTABLE
        );
    }

    #[test]
    fn stable_app_icon_paths_require_revalidation() {
        let uri = Uri::from_static("/images/apps/docker-manager.png");
        assert_eq!(
            cache_control_for(&uri, "images/apps/docker-manager.png"),
            CACHE_REVALIDATE
        );
    }

    #[test]
    fn hashed_vite_assets_are_immutable() {
        let uri = Uri::from_static("/assets/index-ABC123.js");
        assert_eq!(
            cache_control_for(&uri, "assets/index-ABC123.js"),
            CACHE_IMMUTABLE
        );
    }
}
