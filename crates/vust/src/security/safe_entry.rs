//! 安全入口中间件：仅保护登录入口，不改变 API 与静态资源路径。

use crate::models::system_config;
use crate::state::AppState;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose};
use cookie::{Cookie, SameSite, time::Duration};
use std::sync::Arc;

const SAFE_ENTRY_COOKIE: &str = "vust_safe_entry";

/// 安全入口中间件。
pub async fn safe_entry_layer(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    if path.starts_with("/api/") || is_static_asset_path(&path) {
        return next.run(req).await;
    }

    let safe_entry = match system_config::get_safe_entry(&state.metadata_db).await {
        Ok(value) => value,
        Err(err) => {
            tracing::error!("Failed to load safe entry config: {err}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let Some(entry) = safe_entry else {
        if is_safe_entry_like_path(&path) {
            return (StatusCode::FOUND, [(header::LOCATION, "/login")]).into_response();
        }
        return next.run(req).await;
    };

    if path == format!("/{entry}") {
        let mut resp = next.run(req).await;
        resp.headers_mut().insert(
            header::SET_COOKIE,
            build_safe_entry_cookie(&entry)
                .parse()
                .expect("safe entry cookie must be a valid header"),
        );
        return resp;
    }

    if path == "/" && cookie_matches_entry(&headers, &entry) {
        return (StatusCode::FOUND, [(header::LOCATION, format!("/{entry}"))]).into_response();
    }

    if is_safe_entry_like_path(&path) {
        return safe_entry_notice().into_response();
    }

    if path == "/login" && cookie_matches_entry(&headers, &entry) {
        return next.run(req).await;
    }

    if path == "/" || path == "/login" {
        return safe_entry_notice().into_response();
    }

    next.run(req).await
}

fn is_static_asset_path(path: &str) -> bool {
    matches!(
        path.split('/').nth(1),
        Some("assets" | "images" | "favicon.ico" | "robots.txt")
    )
}

fn is_safe_entry_like_path(path: &str) -> bool {
    let value = path.trim_start_matches('/');
    !value.contains('/')
        && (8..=32).contains(&value.len())
        && value.chars().all(|ch| ch.is_ascii_alphanumeric())
}

fn cookie_matches_entry(headers: &HeaderMap, entry: &str) -> bool {
    headers.get_all(header::COOKIE).iter().any(|raw| {
        raw.to_str().ok().is_some_and(|raw| {
            raw.split(';').any(|pair| {
                let mut parts = pair.trim().splitn(2, '=');
                let Some(name) = parts.next() else {
                    return false;
                };
                let Some(value) = parts.next() else {
                    return false;
                };
                name == SAFE_ENTRY_COOKIE
                    && general_purpose::STANDARD
                        .decode(value)
                        .ok()
                        .and_then(|bytes| String::from_utf8(bytes).ok())
                        .is_some_and(|decoded| decoded == entry)
            })
        })
    })
}

fn build_safe_entry_cookie(entry: &str) -> String {
    let value = general_purpose::STANDARD.encode(entry.as_bytes());
    Cookie::build((SAFE_ENTRY_COOKIE, value))
        .path("/")
        .http_only(true)
        .secure(crate::config::get().cookie_secure)
        .same_site(SameSite::Lax)
        .max_age(Duration::days(30))
        .build()
        .to_string()
}

/// 构建清理安全入口 Cookie 的响应头值。
pub fn clear_safe_entry_cookie() -> String {
    Cookie::build((SAFE_ENTRY_COOKIE, ""))
        .path("/")
        .http_only(true)
        .secure(crate::config::get().cookie_secure)
        .same_site(SameSite::Lax)
        .max_age(Duration::seconds(0))
        .build()
        .to_string()
}

fn safe_entry_notice() -> Response {
    let html = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Access Temporarily Unavailable</title>
  <style>
    :root {
      color-scheme: light;
      --bg: #f6f8fb;
      --text: #16212b;
      --muted: #5e7182;
      --border: #d8e1ea;
      --accent: #0f8f7c;
      --accent-strong: #087060;
      --code-bg: #ffffff;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      min-height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 40px 32px;
      font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background: linear-gradient(180deg, #ffffff 0%, var(--bg) 100%);
      color: var(--text);
    }
    main {
      width: min(980px, 100%);
    }
    .topbar {
      display: flex;
      align-items: center;
      gap: 10px;
      margin-bottom: 34px;
    }
    .mark {
      width: 30px;
      height: 30px;
      border-radius: 7px;
      display: grid;
      place-items: center;
      color: #ffffff;
      background: var(--accent);
      font-weight: 800;
      letter-spacing: 0;
    }
    .brand {
      color: #22313d;
      font-size: 14px;
      font-weight: 800;
      letter-spacing: 0;
    }
    h1 {
      margin: 0;
      max-width: 920px;
      font-size: clamp(36px, 6vw, 64px);
      line-height: 1.04;
      letter-spacing: 0;
    }
    p {
      max-width: 760px;
      margin: 20px 0 0;
      color: var(--muted);
      font-size: 17px;
      line-height: 1.7;
    }
    .command {
      display: flex;
      align-items: center;
      gap: 14px;
      margin-top: 34px;
      flex-wrap: wrap;
    }
    .label {
      color: var(--accent-strong);
      font-size: 13px;
      font-weight: 700;
      text-transform: uppercase;
      letter-spacing: 0.08em;
    }
    code {
      display: inline-block;
      padding: 12px 14px;
      border-radius: 6px;
      color: #12312b;
      background: var(--code-bg);
      border: 1px solid var(--border);
      font: 600 14px/1.5 ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
      box-shadow: 0 10px 28px rgba(24, 42, 54, 0.08);
    }
    @media (max-width: 520px) {
      body { padding: 28px 22px; align-items: flex-start; }
      .topbar { margin-top: 18px; margin-bottom: 28px; }
      .command { align-items: flex-start; flex-direction: column; }
    }
  </style>
</head>
<body>
  <main>
    <div class="topbar">
      <div class="mark">S</div>
      <div class="brand">VUST Control Panel</div>
    </div>
    <h1>Access Temporarily Unavailable</h1>
    <p>Secure login access is enabled for this environment. Open the panel with the secure login URL provided by the host command.</p>
    <div class="command">
      <div class="label">View login URL on host</div>
      <code>vustctl info</code>
    </div>
  </main>
</body>
</html>"#;
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}
