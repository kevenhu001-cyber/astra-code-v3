//! axum HTTP server: the official Astra site (embedded static pages) plus the
//! account/auth JSON API. Behavior mirrors the Go `internal/authsrv` server
//! so the existing front-end (HTML/JS) and CLI client work unchanged.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::mailer::Mailer;
use crate::password;
use crate::store::{ApiToken, DeviceGrant, PendingRegistration, Session, Store, User};

// Device-flow constants (RFC 8628 style), matching the Go server.
const DEVICE_EXPIRY_SECS: i64 = 10 * 60;
const DEVICE_INTERVAL: i64 = 5;
const TOKEN_TTL_DAYS: i64 = 30;
const SESSION_TTL_DAYS: i64 = 30;

/// Options configure the server.
#[derive(Clone)]
pub struct Options {
    /// Public base URL used to build verification links. Defaults to the
    /// request host when empty.
    pub base_url: String,
    /// Whether session cookies carry the Secure flag (behind TLS).
    pub cookie_secure: bool,
    /// Session cookie path scope. Defaults to "/".
    pub cookie_path: String,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            base_url: String::new(),
            cookie_secure: false,
            cookie_path: "/".to_string(),
        }
    }
}

/// Shared server state.
#[derive(Clone)]
pub struct Server {
    store: Arc<Store>,
    mailer: Arc<dyn Mailer>,
    opts: Options,
}

impl Server {
    pub fn new(store: Store, mailer: Arc<dyn Mailer>, opts: Options) -> Self {
        Server {
            store: Arc::new(store),
            mailer,
            opts,
        }
    }

    /// Build the full router (static site + API).
    pub fn router(&self) -> Router {
        let state = self.clone();
        Router::new()
            .route("/api/auth/register", post(handlers_register))
            .route("/api/auth/resend-verification", post(handlers_resend))
            .route("/api/auth/verify", get(handlers_verify))
            .route("/api/auth/login", post(handlers_login))
            .route("/api/auth/logout", post(handlers_logout))
            .route("/api/auth/me", get(handlers_me))
            .route("/api/auth/device", post(handlers_device_create))
            .route("/api/auth/device/approve", post(handlers_device_approve))
            .route("/api/auth/device/token", post(handlers_device_token))
            .route(
                "/api/auth/tokens",
                axum::routing::get(handlers_tokens_get)
                    .post(handlers_tokens_post)
                    .delete(handlers_tokens_delete),
            )
            .route("/api/auth/account", post(handlers_account))
            .fallback(Self::static_site)
            .with_state(state)
    }

    /// Serve the embedded static site pages. Unknown paths 404.
    async fn static_site(
        State(_state): State<Server>,
        req: axum::extract::Request,
    ) -> Response {
        let path = req.uri().path().to_string();

        // /assets/* served from the embedded assets directory.
        if let Some(rest) = path.strip_prefix("/assets/") {
            let content = match rest {
                "site.css" => crate::assets::SITE_CSS,
                "auth.js" => crate::assets::AUTH_JS,
                _ => return StatusCode::NOT_FOUND.into_response(),
            };
            let ctype = if rest == "site.css" {
                "text/css; charset=utf-8"
            } else {
                "application/javascript; charset=utf-8"
            };
            return (
                [(header::CONTENT_TYPE, HeaderValue::from_static(ctype))],
                Body::from(content),
            )
                .into_response();
        }

        match path.as_str() {
            "/" => serve_page(crate::assets::INDEX_HTML),
            "/index.html" => redirect_permanent("/"),
            "/login" => serve_page(crate::assets::LOGIN_HTML),
            "/authorize" => serve_page(crate::assets::AUTHORIZE_HTML),
            "/account" => serve_page(crate::assets::ACCOUNT_HTML),
            "/favicon.svg" => (
                [(header::CONTENT_TYPE, HeaderValue::from_static("image/svg+xml"))],
                Body::from(crate::assets::FAVICON_SVG),
            )
                .into_response(),
            "/login.html" => redirect_permanent("/login"),
            "/authorize.html" => redirect_permanent("/authorize"),
            "/account.html" => redirect_permanent("/account"),
            _ => StatusCode::NOT_FOUND.into_response(),
        }
    }
}

fn serve_page(content: &'static str) -> Response {
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"))],
        Body::from(content),
    )
        .into_response()
}

fn redirect_permanent(location: &'static str) -> Response {
    (
        StatusCode::MOVED_PERMANENTLY,
        [(header::LOCATION, HeaderValue::from_static(location))],
    )
        .into_response()
}

// --- helpers ---

fn json_response(code: StatusCode, v: Value) -> Response {
    (code, Json(v)).into_response()
}

fn write_err(code: StatusCode, msg: &str) -> Response {
    json_response(code, json!({ "error": msg }))
}

/// Reject cross-site POSTs (CSRF defense in depth alongside SameSite=Lax).
/// Mirrors the Go check: the Origin's host must match the request Host
/// (port-stripped for host-only comparison).
fn origin_allowed(headers: &HeaderMap, host: &str) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true; // non-browser clients (curl, TUI) carry no Origin
    };
    // Strip scheme:// and any trailing path/query/port.
    let after_scheme = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin);
    let hostish = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    // Strip an IPv6 bracket boundary or a trailing :port.
    let hostish = hostish.strip_prefix('[').unwrap_or(hostish);
    let hostish = hostish.split(':').next().unwrap_or(hostish);
    let host_no_port = host.split(':').next().unwrap_or(host);
    hostish == host_no_port || hostish == host
}

/// Decode a JSON POST body (POST + Origin + JSON gates). Returns the body or
/// an error response.
async fn decode_body<T: for<'de> Deserialize<'de>>(
    method: &Method,
    headers: &HeaderMap,
    host: &str,
    bytes: &[u8],
) -> Result<T, Response> {
    if *method != Method::POST && *method != Method::DELETE {
        return Err(write_err(StatusCode::METHOD_NOT_ALLOWED, "POST required"));
    }
    if !origin_allowed(headers, host) {
        return Err(write_err(StatusCode::FORBIDDEN, "cross-site request rejected"));
    }
    serde_json::from_slice(bytes).map_err(|_| write_err(StatusCode::BAD_REQUEST, "invalid JSON body"))
}

/// Read the sid cookie and return the matching user.
fn session_user(store: &Store, headers: &HeaderMap) -> Option<User> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    let sid = cookie.split(';').find_map(|c| {
        let c = c.trim();
        c.strip_prefix("sid=").map(|v| v.trim().to_string())
    })?;
    let sess = store.find_session(&sid)?;
    store.find_user_by_id(&sess.user_id)
}

/// Authenticate via Authorization: Bearer <api token>.
fn bearer_user(store: &Store, headers: &HeaderMap) -> Option<User> {
    let h = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = h.strip_prefix("Bearer ")?;
    let tok = store.find_token(token)?;
    store.find_user_by_id(&tok.user_id)
}

fn current_user(store: &Store, headers: &HeaderMap) -> Option<User> {
    session_user(store, headers).or_else(|| bearer_user(store, headers))
}

/// Client-safe user shape (never exposes the password hash).
fn to_public(u: &User) -> Value {
    let mut v = json!({
        "id": u.id,
        "email": u.email,
        "created_at": u.created_at.to_rfc3339(),
    });
    if let Some(name) = &u.display_name {
        if !name.is_empty() {
            v["display_name"] = json!(name);
        }
    }
    v
}

fn verify_url(base_url: &str, token: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let base = if base.is_empty() {
        "http://localhost:8080"
    } else {
        base
    };
    format!("{base}/api/auth/verify?token={token}")
}

fn set_session_cookie(
    store: &Store,
    opts: &Options,
    user_id: &str,
) -> Result<HeaderValue, Response> {
    let token = password::random_hex(24);
    let expires = Utc::now() + Duration::days(SESSION_TTL_DAYS);
    store
        .create_session(&Session {
            token: token.clone(),
            user_id: user_id.to_string(),
            expires_at: expires,
        })
        .map_err(|_| write_err(StatusCode::INTERNAL_SERVER_ERROR, "session failed"))?;
    let secure = if opts.cookie_secure { "; Secure" } else { "" };
    let value = format!(
        "sid={token}; Path={}; HttpOnly; SameSite=Lax{secure}; Max-Age={}",
        opts.cookie_path,
        SESSION_TTL_DAYS * 24 * 3600
    );
    HeaderValue::from_str(&value)
        .map_err(|_| write_err(StatusCode::INTERNAL_SERVER_ERROR, "session failed"))
}

fn clear_session_cookie(opts: &Options) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "sid=; Path={}; HttpOnly; SameSite=Lax; Max-Age=-1",
        opts.cookie_path
    ))
    .unwrap()
}

// --- registration ---

async fn handlers_register(
    State(s): State<Server>,
    req: axum::extract::Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts.headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    #[derive(Deserialize)]
    struct Body {
        email: String,
        password: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if let Err(e) = password::validate_credentials(&body.email, &body.password) {
        return write_err(StatusCode::BAD_REQUEST, &e);
    }
    let email = crate::store::normalize_email(&body.email);

    // Anti-enumeration: always return ok.
    if s.store.find_user_by_email(&email).is_some() {
        return json_response(StatusCode::OK, json!({ "ok": true }));
    }

    let hash = match password::hash_password(&body.password) {
        Ok(h) => h,
        Err(e) => return write_err(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    let token = password::random_hex(16);
    let now = Utc::now();
    if let Err(_) = s.store.upsert_pending(&PendingRegistration {
        email: email.clone(),
        password_hash: hash,
        token: token.clone(),
        expires_at: now + Duration::hours(24),
    }) {
        return write_err(StatusCode::INTERNAL_SERVER_ERROR, "storage failed");
    }
    let url = verify_url(&s.opts.base_url, &token);
    if let Err(e) = s.mailer.send_verification(&email, &url) {
        tracing::warn!(email = %email, error = %e, "verification email failed");
    }
    json_response(StatusCode::OK, json!({ "ok": true }))
}

async fn handlers_resend(
    State(s): State<Server>,
    req: axum::extract::Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts.headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    #[derive(Deserialize)]
    struct Body {
        email: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let email = crate::store::normalize_email(&body.email);
    if let Some(p) = s.store.find_pending(&email) {
        if p.expires_at > Utc::now() {
            let url = verify_url(&s.opts.base_url, &p.token);
            let _ = s.mailer.send_verification(&email, &url);
        }
    }
    json_response(StatusCode::OK, json!({ "ok": true }))
}

async fn handlers_verify(State(s): State<Server>, Query(q): Query<VerifyQuery>) -> Response {
    #[derive(Deserialize)]
    struct VerifyQuery {
        token: String,
    }
    let token = &q.token;
    let p = match s.store.find_pending_by_token(token) {
        Some(p) => p,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "Verification link is invalid or expired. Please register again.",
            )
                .into_response()
        }
    };
    if p.expires_at < Utc::now() {
        return (
            StatusCode::BAD_REQUEST,
            "Verification link is invalid or expired. Please register again.",
        )
            .into_response();
    }
    // Register is idempotent: the user may have verified in another tab.
    let user = match s.store.find_user_by_email(&p.email) {
        Some(u) => u,
        None => {
            let u = User {
                id: password::random_hex(8),
                email: p.email.clone(),
                password_hash: p.password_hash.clone(),
                display_name: None,
                created_at: Utc::now(),
                verified_at: Some(Utc::now()),
            };
            if let Err(_) = s.store.create_user(&u) {
                return write_err(StatusCode::INTERNAL_SERVER_ERROR, "storage failed");
            }
            u
        }
    };
    let _ = s.store.delete_pending(&p.email);
    let cookie = match set_session_cookie(&s.store, &s.opts, &user.id) {
        Ok(c) => c,
        Err(r) => return r,
    };
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, HeaderValue::from_static("/account")),
            (header::SET_COOKIE, cookie),
        ],
    )
        .into_response()
}

// --- login / logout / me ---

async fn handlers_login(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts.headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    #[derive(Deserialize)]
    struct Body {
        email: String,
        password: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let user = s.store.find_user_by_email(&body.email);
    let Some(user) = user else {
        return write_err(StatusCode::UNAUTHORIZED, "invalid email or password");
    };
    if !password::check_password(&user.password_hash, &body.password) {
        return write_err(StatusCode::UNAUTHORIZED, "invalid email or password");
    }
    let cookie = match set_session_cookie(&s.store, &s.opts, &user.id) {
        Ok(c) => c,
        Err(r) => return r,
    };
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "user": to_public(&user) })),
    )
        .into_response()
}

async fn handlers_logout(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let headers = req.headers().clone();
    if let Some(cookie) = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(sid) = cookie
            .split(';')
            .find_map(|c| c.trim().strip_prefix("sid=").map(|v| v.trim().to_string()))
        {
            let _ = s.store.delete_session(&sid);
        }
    }
    (
        StatusCode::OK,
        [(header::SET_COOKIE, clear_session_cookie(&s.opts))],
        Json(json!({ "ok": true })),
    )
        .into_response()
}

async fn handlers_me(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let headers = req.headers().clone();
    let user = current_user(&s.store, &headers);
    match user {
        Some(u) => json_response(StatusCode::OK, json!({ "user": to_public(&u) })),
        // 200 + user:null instead of 401 (the login/account pages rely on it).
        None => json_response(StatusCode::OK, json!({ "user": null })),
    }
}

// --- device flow ---

async fn handlers_device_create(State(s): State<Server>, _req: axum::extract::Request) -> Response {
    // No auth needed to start a grant (RFC 8628); the user authorizes in the
    // browser with their session.
    let g = DeviceGrant {
        device_code: password::random_hex(16),
        user_code: password::random_user_code(),
        status: "pending".to_string(),
        user_id: None,
        token: None,
        created_at: Utc::now(),
        expires_at: Utc::now() + Duration::seconds(DEVICE_EXPIRY_SECS),
        approved_at: None,
    };
    if let Err(e) = s.store.create_device(&g) {
        return write_err(StatusCode::INTERNAL_SERVER_ERROR, "storage failed");
    }
    let base = {
        let b = s.opts.base_url.trim_end_matches('/');
        if b.is_empty() {
            "http://localhost:8080"
        } else {
            b
        }
    };
    json_response(
        StatusCode::OK,
        json!({
            "device_code": g.device_code,
            "user_code": g.user_code,
            "verification_uri": format!("{base}/authorize?code={}", g.user_code),
            "expires_in": DEVICE_EXPIRY_SECS,
            "interval": DEVICE_INTERVAL,
        }),
    )
}

async fn handlers_device_approve(
    State(s): State<Server>,
    req: axum::extract::Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts.headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let user = current_user(&s.store, &parts.headers);
    let Some(user) = user else {
        return write_err(StatusCode::UNAUTHORIZED, "please log in first");
    };
    #[derive(Deserialize)]
    struct Body {
        #[serde(rename = "user_code")]
        user_code: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let g = s.store.find_device_by_user_code(&body.user_code);
    let Some(g) = g else {
        return write_err(StatusCode::BAD_REQUEST, "invalid or expired code");
    };
    if g.status != "pending" || g.expires_at < Utc::now() {
        return write_err(StatusCode::BAD_REQUEST, "invalid or expired code");
    }
    let token = ApiToken {
        token: password::random_hex(24),
        user_id: user.id.clone(),
        label: Some("device".to_string()),
        created_at: Utc::now(),
        expires_at: Utc::now() + Duration::days(TOKEN_TTL_DAYS),
    };
    if let Err(_) = s.store.create_token(&token) {
        return write_err(StatusCode::INTERNAL_SERVER_ERROR, "storage failed");
    }
    let now = Utc::now();
    let token_str = token.token.clone();
    let user_id = user.id.clone();
    let _ = s.store.update_device(&g.device_code, |d| {
        d.status = "approved".to_string();
        d.user_id = Some(user_id);
        d.token = Some(token_str);
        d.approved_at = Some(now);
    });
    json_response(StatusCode::OK, json!({ "ok": true }))
}

async fn handlers_device_token(
    State(s): State<Server>,
    req: axum::extract::Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts.headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    #[derive(Deserialize)]
    struct Body {
        #[serde(rename = "device_code")]
        device_code: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let g = match s.store.find_device_by_code(&body.device_code) {
        Some(g) => g,
        None => return json_response(StatusCode::OK, json!({ "status": "expired" })),
    };
    if g.expires_at < Utc::now() {
        return json_response(StatusCode::OK, json!({ "status": "expired" }));
    }
    match g.status.as_str() {
        "pending" => json_response(StatusCode::OK, json!({ "status": "pending" })),
        "approved" => {
            let Some(user) = g.user_id.as_ref().and_then(|id| s.store.find_user_by_id(id)) else {
                return json_response(StatusCode::OK, json!({ "status": "expired" }));
            };
            json_response(
                StatusCode::OK,
                json!({
                    "status": "approved",
                    "access_token": g.token,
                    "user": to_public(&user),
                }),
            )
        }
        _ => json_response(StatusCode::OK, json!({ "status": "expired" })),
    }
}

// --- api tokens (account page) ---

async fn handlers_tokens_get(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let headers = req.headers().clone();
    let Some(user) = current_user(&s.store, &headers) else {
        return write_err(StatusCode::UNAUTHORIZED, "not authenticated");
    };
    let toks = s.store.tokens_for_user(&user.id);
    let out: Vec<Value> = toks
        .iter()
        .map(|t| {
            json!({
                "token": t.token,
                "label": t.label,
                "created_at": t.created_at.to_rfc3339(),
                "expires_at": t.expires_at.to_rfc3339(),
            })
        })
        .collect();
    json_response(StatusCode::OK, json!({ "tokens": out }))
}

async fn handlers_tokens_post(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let headers = req.headers().clone();
    let Some(user) = current_user(&s.store, &headers) else {
        return write_err(StatusCode::UNAUTHORIZED, "not authenticated");
    };
    let tok = ApiToken {
        token: password::random_hex(24),
        user_id: user.id,
        label: Some("manual".to_string()),
        created_at: Utc::now(),
        expires_at: Utc::now() + Duration::days(TOKEN_TTL_DAYS),
    };
    if let Err(_) = s.store.create_token(&tok) {
        return write_err(StatusCode::INTERNAL_SERVER_ERROR, "storage failed");
    }
    json_response(StatusCode::OK, json!({ "token": tok.token }))
}

async fn handlers_tokens_delete(
    State(s): State<Server>,
    req: axum::extract::Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts.headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let Some(_user) = current_user(&s.store, &parts.headers) else {
        return write_err(StatusCode::UNAUTHORIZED, "not authenticated");
    };
    #[derive(Deserialize)]
    struct Body {
        token: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let _ = s.store.revoke_token(&body.token);
    json_response(StatusCode::OK, json!({ "ok": true }))
}

// --- account update ---

async fn handlers_account(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts.headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let Some(mut user) = current_user(&s.store, &parts.headers) else {
        return write_err(StatusCode::UNAUTHORIZED, "not authenticated");
    };
    #[derive(Deserialize)]
    struct Body {
        #[serde(rename = "display_name")]
        display_name: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let name = body.display_name.trim();
    let name: String = name.chars().take(60).collect();
    let user_id = user.id.clone();
    if let Err(_) = s.store.update_user(&user_id, |u| {
        u.display_name = Some(name.clone());
    }) {
        return write_err(StatusCode::INTERNAL_SERVER_ERROR, "update failed");
    }
    user.display_name = Some(name);
    json_response(StatusCode::OK, json!({ "user": to_public(&user) }))
}
