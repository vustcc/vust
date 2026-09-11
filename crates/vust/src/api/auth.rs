//! 认证 API：单用户管理员登录、服务端会话校验与退出。

use crate::models::auth_sessions::{
    AuthSessionRecord, CreateAuthSessionInput, create_auth_session, get_auth_session_by_hash,
    hash_session_token, is_session_active, revoke_auth_session_by_token, touch_auth_session,
};
use crate::models::logging::LogModule;
use crate::models::user::User;
use crate::services::logging::{self, OperationEventBuilder};
use crate::state::AppState;
use crate::types::{ApiError, ApiResponse, ApiResult, AuthError};
use axum::{
    Json, Router,
    extract::{FromRef, FromRequestParts, Request, State, connect_info::ConnectInfo},
    http::{HeaderMap, StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::{Cookie as AxumCookie, SameSite};
use bcrypt::verify;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use vust_contracts::api::ErrorCode;
use vust_contracts::auth::AuthBody;

const SESSION_COOKIE: &str = "vust_session";
const SESSION_COOKIE_MAX_AGE_SECONDS: i64 = 12 * 60 * 60;
const SESSION_COOKIE_PATH: &str = "/";
const LEGACY_SESSION_COOKIE_PATHS: &[&str] = &["/api/v1", "/api/v1/auth"];

/// 已通过服务端会话校验的管理员身份。
#[derive(Debug, Clone)]
pub struct AuthenticatedAdmin {
    pub id: i64,
    pub username: String,
    pub session: AuthSessionRecord,
}

/// 登录请求载荷。
#[derive(Debug, Deserialize)]
pub struct AuthPayload {
    pub username: String,
    pub password: String,
    pub captcha_id: Option<String>,
    pub captcha: Option<String>,
}

/// 登录失败扩展信息。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginFailurePayload {
    pub captcha_required: bool,
    pub locked: bool,
    pub lockout_remaining: Option<u64>,
}

/// 当前客户端验证码状态。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptchaStatusPayload {
    pub captcha_required: bool,
    pub locked: bool,
    pub lockout_remaining: Option<u64>,
}

/// 处理管理员登录请求，成功后创建可并存的服务端会话。
pub async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(conn): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(payload): Json<AuthPayload>,
) -> ApiResult<Response> {
    let client_ip = conn.ip();
    let pool = &state.metadata_db;
    let trace_id = logging::resolve_trace_id(&headers);

    if let Some(remaining) = state.login_tracker.lockout_remaining_secs(&client_ip).await {
        return Ok(login_failure_response(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many login attempts. Please try again later.",
            "api.auth.loginLocked",
            true,
            true,
            Some(remaining),
        ));
    }

    if payload.username.is_empty() || payload.password.is_empty() {
        logging::operation_log_failure("anonymous", "user_login", client_ip)
            .module(LogModule::Auth)
            .trace_id(&trace_id)
            .source("vust_api")
            .request("POST", "/api/v1/auth/login")
            .metadata(json!({
                "reason": "MissingCredentials",
                "auth_stage": "input_validate",
                "attempted_user": &payload.username,
                "message_key": "platformLog.auth.login.missingCredentials"
            }))
            .finish(pool);
        return Err(ApiError::from(AuthError::MissingCredentials));
    }

    if state.login_tracker.needs_captcha(&client_ip).await {
        match (&payload.captcha_id, &payload.captcha) {
            (Some(captcha_id), Some(captcha)) => {
                if !state.captcha_service.verify(captcha_id, captcha).await {
                    let newly_locked = state.login_tracker.record_failure(&client_ip).await;
                    return Ok(login_failure_after_failure(
                        &state,
                        client_ip,
                        &payload.username,
                        newly_locked,
                        "The captcha is invalid. Please try again.",
                        "api.auth.captchaInvalid",
                    )
                    .await);
                }
            }
            _ => {
                return Ok(login_failure_response(
                    StatusCode::UNAUTHORIZED,
                    "Captcha is required.",
                    "api.auth.captchaRequired",
                    true,
                    false,
                    None,
                ));
            }
        }
    }

    let user_record = sqlx::query_as::<_, User>(
        r#"
        SELECT id, username, password_hash, status, password_changed_at, created_at, updated_at
        FROM users
        WHERE username = ?1
        "#,
    )
    .bind(&payload.username)
    .fetch_one(pool)
    .await
    .map_err(|err| {
        let log_reason = match err {
            sqlx::Error::RowNotFound => "UserNotFound",
            _ => "DatabaseError",
        };

        logging::operation_log_failure("anonymous", "user_login", client_ip)
            .module(LogModule::Auth)
            .trace_id(&trace_id)
            .source("vust_api")
            .request("POST", "/api/v1/auth/login")
            .metadata(json!({
                "reason": log_reason,
                "auth_stage": "user_lookup",
                "attempted_user": &payload.username,
                "message_key": "platformLog.auth.login.userLookupFailed"
            }))
            .finish(pool);

        match err {
            sqlx::Error::RowNotFound => ApiError::from(AuthError::WrongCredentials),
            _ => err.into(),
        }
    });
    let user_record = match user_record {
        Ok(user) => user,
        Err(err) => {
            if err.code == ErrorCode::AuthWrongCredentials {
                let newly_locked = state.login_tracker.record_failure(&client_ip).await;
                return Ok(login_failure_after_failure(
                    &state,
                    client_ip,
                    &payload.username,
                    newly_locked,
                    "The username or password is incorrect.",
                    "api.auth.wrongCredentials",
                )
                .await);
            }
            return Err(err);
        }
    };

    if !user_record.is_active() {
        logging::operation_log_failure("anonymous", "user_login", client_ip)
            .module(LogModule::Auth)
            .user_id(user_record.id)
            .trace_id(&trace_id)
            .source("vust_api")
            .request("POST", "/api/v1/auth/login")
            .metadata(json!({
                "reason": "AdminDisabled",
                "auth_stage": "user_status",
                "attempted_user": &payload.username,
                "message_key": "platformLog.auth.login.userLookupFailed"
            }))
            .finish(pool);
        return Err(ApiError::from(AuthError::WrongCredentials));
    }

    let valid_password = verify(&payload.password, &user_record.password_hash).map_err(|err| {
        logging::operation_log_failure("anonymous", "user_login", client_ip)
            .module(LogModule::Auth)
            .user_id(user_record.id)
            .trace_id(&trace_id)
            .source("vust_api")
            .request("POST", "/api/v1/auth/login")
            .metadata(json!({
                "reason": "BcryptInternalError",
                "auth_stage": "password_verify",
                "error_detail": err.to_string(),
                "attempted_user": &payload.username,
                "target_user_id": user_record.id,
                "target_username": &user_record.username,
                "message_key": "platformLog.auth.login.passwordVerifyError"
            }))
            .finish(pool);

        tracing::error!(
            "Bcrypt verification internal error for user {}: {:?}",
            user_record.username,
            err
        );
        ApiError::from(AuthError::WrongCredentials)
    })?;

    if !valid_password {
        logging::operation_log_failure("anonymous", "user_login", client_ip)
            .module(LogModule::Auth)
            .user_id(user_record.id)
            .trace_id(&trace_id)
            .source("vust_api")
            .request("POST", "/api/v1/auth/login")
            .metadata(json!({
                "reason": "InvalidPassword",
                "auth_stage": "password_verify",
                "attempted_user": &payload.username,
                "target_user_id": user_record.id,
                "target_username": &user_record.username,
                "message_key": "platformLog.auth.login.invalidPassword"
            }))
            .finish(pool);
        let newly_locked = state.login_tracker.record_failure(&client_ip).await;
        return Ok(login_failure_after_failure(
            &state,
            client_ip,
            &payload.username,
            newly_locked,
            "The username or password is incorrect.",
            "api.auth.wrongCredentials",
        )
        .await);
    }

    let created = create_auth_session(
        pool,
        CreateAuthSessionInput {
            user_id: user_record.id,
            client_ip: Some(client_ip.to_string()),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.to_string()),
        },
    )
    .await
    .map_err(|err| ApiError::Internal(err.to_string()))?;

    logging::operation_log_success(&payload.username, "user_login", client_ip)
        .module(LogModule::Auth)
        .user_id(user_record.id)
        .trace_id(&trace_id)
        .source("vust_api")
        .request("POST", "/api/v1/auth/login")
        .metadata(json!({
            "message_key": "platformLog.auth.login.success",
            "session_id": created.record.id
        }))
        .finish(pool);

    state.login_tracker.clear_failures(&client_ip).await;

    let mut body = ApiResponse::success_with_raw(
        "Login succeeded",
        Some(AuthBody::new(
            user_record.id,
            user_record.username,
            created.record.id,
            created.record.expires_at,
        )),
    );
    body.message_key = Some("api.auth.loginSuccess".to_string());
    let mut resp = body.into_response();

    set_auth_cookie(&mut resp, &created.token);
    Ok(resp)
}

/// 获取新的验证码。
pub async fn captcha(State(state): State<Arc<AppState>>) -> ApiResult<impl IntoResponse> {
    let payload = state.captcha_service.generate().await;
    let mut body = ApiResponse::success_with_raw("Captcha generated", payload);
    body.message_key = Some("api.auth.captchaGenerated".to_string());
    Ok(body.into_response())
}

/// 查询当前 IP 是否需要验证码。
pub async fn captcha_status(
    State(state): State<Arc<AppState>>,
    ConnectInfo(conn): ConnectInfo<SocketAddr>,
) -> ApiResult<impl IntoResponse> {
    let lockout_remaining = state.login_tracker.lockout_remaining_secs(&conn.ip()).await;
    let payload = CaptchaStatusPayload {
        captcha_required: state.login_tracker.needs_captcha(&conn.ip()).await,
        locked: lockout_remaining.is_some(),
        lockout_remaining,
    };
    let mut body = ApiResponse::success_with_raw("Captcha status loaded", payload);
    body.message_key = Some("api.auth.captchaStatusLoaded".to_string());
    Ok(body.into_response())
}

/// 返回当前有效管理员会话。
pub async fn me(admin: AuthenticatedAdmin) -> ApiResult<impl IntoResponse> {
    let mut body = ApiResponse::success_with_raw(
        "Current session loaded",
        Some(AuthBody::new(
            admin.id,
            admin.username,
            admin.session.id,
            admin.session.expires_at,
        )),
    );
    body.message_key = Some("api.auth.currentSessionLoaded".to_string());
    Ok(body.into_response())
}

/// 撤销当前认证会话并清理 Cookie。
pub async fn logout(
    State(state): State<Arc<AppState>>,
    ConnectInfo(conn): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let trace_id = logging::resolve_trace_id(&headers);
    let token = extract_cookie_from_headers(&headers, SESSION_COOKIE);

    if let Some(token) = token {
        let token_hash = hash_session_token(&token);
        let session = get_auth_session_by_hash(&state.metadata_db, &token_hash).await?;
        if let Some(session) = session {
            let user = get_active_admin_by_id(&state.metadata_db, session.user_id)
                .await
                .ok()
                .flatten();
            let username = user
                .as_ref()
                .map(|value| value.username.as_str())
                .unwrap_or("anonymous");
            let mut platform_log = OperationEventBuilder::new(username, "user_logout", conn.ip())
                .module(LogModule::Auth)
                .trace_id(&trace_id)
                .source("vust_api")
                .request("POST", "/api/v1/auth/logout")
                .set_success()
                .metadata(json!({
                    "message_key": "platformLog.auth.logout.success",
                    "session_id": session.id
                }));
            if let Some(user) = user {
                platform_log = platform_log.user_id(user.id);
            }
            platform_log.finish(&state.metadata_db);

            revoke_auth_session_by_token(&state.metadata_db, &token, "logout").await?;
        } else {
            log_logout_without_session(&state, conn.ip(), &trace_id);
        }
    } else {
        log_logout_without_session(&state, conn.ip(), &trace_id);
    }

    let mut body = ApiResponse::success_with_raw("Logout succeeded", ());
    body.message_key = Some("api.auth.logoutSuccess".to_string());
    let mut resp = body.into_response();
    clear_auth_cookie(&mut resp);
    Ok(resp)
}

/// 创建认证相关路由。
pub fn auth_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/login", post(login))
        .route("/me", get(me))
        .route("/logout", post(logout))
        .route("/captcha-status", get(captcha_status))
}

fn login_failure_response(
    status: StatusCode,
    message: &str,
    message_key: &str,
    captcha_required: bool,
    locked: bool,
    lockout_remaining: Option<u64>,
) -> Response {
    let payload = LoginFailurePayload {
        captcha_required,
        locked,
        lockout_remaining,
    };
    let mut body = ApiResponse::error_with_raw(message, payload).set_code(status.as_u16());
    body.message_key = Some(message_key.to_string());
    (status, Json(body)).into_response()
}

async fn login_failure_after_failure(
    state: &AppState,
    client_ip: std::net::IpAddr,
    attempted_username: &str,
    newly_locked: bool,
    message: &str,
    message_key: &str,
) -> Response {
    if let Some(remaining) = state.login_tracker.lockout_remaining_secs(&client_ip).await {
        if newly_locked {
            crate::services::logging::OperationEventBuilder::new(
                "anonymous",
                "login_lockout",
                client_ip,
            )
            .module(LogModule::Auth)
            .outcome(vust_contracts::logging::OperationOutcome::Failure)
            .metadata(json!({
                "username": attempted_username,
                "clientIp": client_ip.to_string(),
                "lockoutSeconds": remaining,
            }))
            .finish(&state.metadata_db);
        }
        return login_failure_response(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many login attempts. Please try again later.",
            "api.auth.loginLocked",
            true,
            true,
            Some(remaining),
        );
    }
    login_failure_response(
        StatusCode::UNAUTHORIZED,
        message,
        message_key,
        true,
        false,
        None,
    )
}

impl<S> FromRequestParts<S> for AuthenticatedAdmin
where
    Arc<AppState>: axum::extract::FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        if let Some(admin) = parts.extensions.get::<AuthenticatedAdmin>() {
            return Ok(admin.clone());
        }
        let state = Arc::<AppState>::from_ref(state);
        authenticate_parts(parts, &state).await
    }
}

/// 认证中间件，用于在进入受保护路由前统一阻断无效会话。
pub async fn auth_layer(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> ApiResult<Response> {
    let path = req.uri().path();
    // 升级期间，如果是普通业务请求则直接拦截，但需要放行 upgrades 相关的状态轮询接口
    if !is_upgrade_api_path(path) && is_system_upgrading(&state.metadata_db).await {
        return Err(ApiError::new(
            axum::http::StatusCode::LOCKED,
            vust_contracts::api::ErrorCode::AuthForbidden,
            "System is currently upgrading, please wait.",
        )
        .with_message_key("api.upgrades.systemIsUpgrading"));
    }

    let (mut parts, body) = req.into_parts();
    let admin = authenticate_parts(&mut parts, &state).await?;
    parts.extensions.insert(admin);
    let req = Request::from_parts(parts, body);
    Ok(next.run(req).await)
}

fn is_upgrade_api_path(path: &str) -> bool {
    path.starts_with("/api/v1/upgrades/") || path.starts_with("/upgrades/")
}

async fn is_system_upgrading(pool: &sqlx::SqlitePool) -> bool {
    let result: Result<i64, _> =
        sqlx::query_scalar("SELECT COUNT(*) FROM upgrade_plans WHERE status = 'running'")
            .fetch_one(pool)
            .await;
    result.unwrap_or(0) > 0
}

async fn authenticate_parts(
    parts: &mut Parts,
    state: &Arc<AppState>,
) -> Result<AuthenticatedAdmin, ApiError> {
    let token = extract_cookie(parts, SESSION_COOKIE).inspect_err(|_| {
        tracing::warn!("Authentication failed: session cookie is missing");
    })?;
    let token_hash = hash_session_token(&token);
    let session = get_auth_session_by_hash(&state.metadata_db, &token_hash).await?;
    let Some(session) = session else {
        tracing::warn!(
            token_hash_prefix = %short_token_hash(&token_hash),
            "Authentication failed: session token was not found"
        );
        return Err(ApiError::from(AuthError::InvalidToken));
    };
    if !is_session_active(&session) {
        tracing::warn!(
            session_id = %session.id,
            revoked = session.revoked_at.is_some(),
            expires_at = %session.expires_at,
            "Authentication failed: session is inactive"
        );
        return Err(ApiError::from(AuthError::InvalidToken));
    }

    let user = get_active_admin_by_id(&state.metadata_db, session.user_id).await?;
    let Some(user) = user else {
        tracing::warn!(
            session_id = %session.id,
            user_id = session.user_id,
            "Authentication failed: session user is missing or inactive"
        );
        return Err(ApiError::from(AuthError::InvalidToken));
    };
    touch_auth_session(&state.metadata_db, &session.id).await?;

    Ok(AuthenticatedAdmin {
        id: user.id,
        username: user.username,
        session,
    })
}

fn short_token_hash(value: &str) -> String {
    value.chars().take(12).collect()
}

async fn get_active_admin_by_id(
    pool: &sqlx::SqlitePool,
    user_id: i64,
) -> sqlx::Result<Option<User>> {
    let user = sqlx::query_as::<_, User>(
        r#"
        SELECT id, username, password_hash, status, password_changed_at, created_at, updated_at
        FROM users
        WHERE id = ?1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(user.filter(User::is_active))
}

fn log_logout_without_session(state: &AppState, client_ip: std::net::IpAddr, trace_id: &str) {
    OperationEventBuilder::new("anonymous", "user_logout", client_ip)
        .module(LogModule::Auth)
        .trace_id(trace_id)
        .source("vust_api")
        .request("POST", "/api/v1/auth/logout")
        .set_success()
        .metadata(json!({ "message_key": "platformLog.auth.logout.noSession" }))
        .finish(&state.metadata_db);
}

fn build_cookie(name: &str, value: &str, max_age_seconds: i64, path: &str) -> AxumCookie<'static> {
    let mut cookie = AxumCookie::build((name.to_string(), value.to_string()))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path(path.to_string())
        .max_age(cookie::time::Duration::seconds(max_age_seconds))
        .build();
    if crate::config::get().cookie_secure {
        cookie.set_secure(true);
    }
    cookie
}

fn set_auth_cookie(resp: &mut Response, token: &str) {
    clear_auth_cookie(resp);
    let cookie = build_cookie(
        SESSION_COOKIE,
        token,
        SESSION_COOKIE_MAX_AGE_SECONDS,
        SESSION_COOKIE_PATH,
    );
    resp.headers_mut()
        .append(header::SET_COOKIE, cookie.to_string().parse().unwrap());
}

fn clear_auth_cookie(resp: &mut Response) {
    for path in LEGACY_SESSION_COOKIE_PATHS {
        clear_auth_cookie_path(resp, path);
    }
    clear_auth_cookie_path(resp, SESSION_COOKIE_PATH);
}

fn clear_auth_cookie_path(resp: &mut Response, path: &str) {
    let cookie = build_cookie(SESSION_COOKIE, "", 0, path);
    resp.headers_mut()
        .append(header::SET_COOKIE, cookie.to_string().parse().unwrap());
}

fn extract_cookie(parts: &Parts, cookie_name: &str) -> Result<String, ApiError> {
    extract_cookie_from_headers(&parts.headers, cookie_name)
        .ok_or_else(|| ApiError::from(AuthError::InvalidToken))
}

fn extract_cookie_from_headers(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    let mut matched = None;
    for value in headers.get_all(header::COOKIE) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for part in value.split(';') {
            let trimmed = part.trim();
            if let Some((name, val)) = trimmed.split_once('=')
                && name == cookie_name
            {
                matched = Some(val.to_string());
            }
        }
    }
    matched
}
