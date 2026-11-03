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
use crate::store::{ApiToken, DeviceGrant, ModelRoute, PendingRegistration, Session, Store, User};

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine as _;
use bytes::Bytes;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

// Device-flow constants (RFC 8628 style), matching the Go server.
const DEVICE_EXPIRY_SECS: i64 = 10 * 60;
const DEVICE_INTERVAL: i64 = 5;
const TOKEN_TTL_DAYS: i64 = 30;
const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const SESSION_TTL_DAYS: i64 = 30;

/// Interstitial page shown after the emailed verification link is opened.
///
/// Mirrors the site's dark visual language (black page, hairline card, one
/// accent dot) so the hand-off from email to account does not flash an
/// unstyled white screen. `__SID__` is replaced with the new session token
/// before the response is written.
const VERIFY_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Email verified &mdash; Astra</title>
<link rel="icon" href="/favicon.svg" type="image/svg+xml">
<script>try{document.documentElement.dataset.theme=localStorage.getItem('astra_theme')||'light'}catch(e){document.documentElement.dataset.theme='light'}</script>
<style>
  :root { color-scheme: light; --bg:#fff; --surface:#fff; --text:#17191c; --line:rgba(18,22,28,.11); --muted:rgba(23,25,28,.54); }
  :root[data-theme="dark"] { color-scheme: dark; --bg:#000; --surface:#0a0a0a; --text:#fff; --line:rgba(255,255,255,.08); --muted:rgba(255,255,255,.5); }
  html, body { margin: 0; }
  body {
    min-height: 100vh;
    display: flex; align-items: center; justify-content: center;
    padding: 24px;
    background: var(--bg); color: var(--text);
    font-family: "Geist", "Inter", -apple-system, BlinkMacSystemFont, "Helvetica Neue", Arial, sans-serif;
    -webkit-font-smoothing: antialiased;
  }
  .card {
    width: min(420px, 100%);
    padding: 36px;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 20px;
    text-align: center;
  }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: #86c46b; margin: 0 auto 18px; }
  h1 { margin: 0 0 8px; font-size: 22px; font-weight: 600; letter-spacing: -.025em; }
  p { margin: 0; color: var(--muted); font-size: 14px; line-height: 1.6; }
  a { color: var(--text); }
</style>
</head>
<body>
  <div class="card">
    <div class="dot"></div>
    <h1>Email verified</h1>
    <p>Taking you to your account&hellip;</p>
  </div>
  <script>try{localStorage.setItem('astra_sid','__SID__');localStorage.setItem('astra_token','__SID__');}catch(e){}location.replace('/account');</script>
  <noscript><p><a href="/account">Continue to your account</a></p></noscript>
</body>
</html>"#;

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

#[derive(Debug, Clone)]
struct OidcCode {
    user_id: String,
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    code_challenge_method: String,
    nonce: String,
    expires_at: chrono::DateTime<Utc>,
}

/// Shared server state.
#[derive(Clone)]
pub struct Server {
    store: Arc<Store>,
    mailer: Arc<dyn Mailer>,
    opts: Options,
    oidc_codes: Arc<Mutex<HashMap<String, OidcCode>>>,
    oidc_jwt_secret: Arc<String>,
    admin_email: String,
    http: reqwest::Client,
}

impl Server {
    pub fn new(store: Store, mailer: Arc<dyn Mailer>, opts: Options) -> Self {
        Self::from_arc(Arc::new(store), mailer, opts)
    }

    /// Construct from a shared store handle, so callers that hold the store
    /// (e.g. tests) observe mutations made by the server.
    pub fn from_arc(store: Arc<Store>, mailer: Arc<dyn Mailer>, opts: Options) -> Self {
        let secret = password::random_hex(32);
        let admin_email = std::env::var("ASTRA_AUTH_ADMIN_EMAIL")
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        if let Err(err) = store.configure_admin_email(&admin_email) {
            tracing::error!("failed to configure administrator account: {err}");
        }
        Server {
            store,
            mailer,
            opts,
            oidc_codes: Arc::new(Mutex::new(HashMap::new())),
            oidc_jwt_secret: Arc::new(secret),
            admin_email,
            http: reqwest::Client::new(),
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
            .route("/api/auth/device/generate", post(handlers_device_generate))
            .route("/api/auth/device/approve", post(handlers_device_approve))
            .route("/api/auth/device/token", post(handlers_device_token))
            .route("/api/auth/device/consume", post(handlers_device_consume))
            .route(
                "/api/auth/tokens",
                axum::routing::get(handlers_tokens_get)
                    .post(handlers_tokens_post)
                    .delete(handlers_tokens_delete),
            )
            .route("/api/auth/account", post(handlers_account))
            .route("/api/auth/balance", get(handlers_balance))
            .route(
                "/api/admin/models",
                get(handlers_admin_models_get)
                    .post(handlers_admin_models_post)
                    .delete(handlers_admin_models_delete),
            )
            .route("/api/admin/users", get(handlers_admin_users))
            .route("/api/admin/users/balance", post(handlers_admin_balance))
            .route("/v1/models", get(handlers_gateway_models))
            .route("/v1/chat/completions", post(handlers_gateway_chat))
            .route(
                "/.well-known/openid-configuration",
                get(Self::handlers_oidc_discovery),
            )
            .route("/.well-known/jwks.json", get(Self::handlers_jwks))
            .route("/authorize", get(Self::handlers_authorize))
            .route("/token", post(Self::handlers_token))
            .route("/userinfo", get(Self::handlers_userinfo))
            .fallback(Self::static_site)
            .with_state(state)
    }

    async fn handlers_oidc_discovery(State(s): State<Server>) -> Response {
        let base = s.opts.base_url.trim_end_matches('/').to_string();
        let base = if base.is_empty() {
            "http://localhost:8080".to_string()
        } else {
            base
        };
        json_response(
            StatusCode::OK,
            json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "userinfo_endpoint": format!("{base}/userinfo"),
                "jwks_uri": format!("{base}/.well-known/jwks.json"),
                "scopes_supported": ["openid","profile","email","offline_access","api:access","grok-cli:access"],
                "response_types_supported": ["code"],
                "grant_types_supported": ["authorization_code","refresh_token","urn:ietf:params:oauth:grant-type:device_code"],
                "code_challenge_methods_supported": ["S256", "plain"],
                "id_token_signing_alg_values_supported": ["RS256", "HS256"]
            }),
        )
    }

    async fn handlers_jwks() -> Response {
        json_response(
            StatusCode::OK,
            json!({
                "keys": []
            }),
        )
    }

    async fn handlers_userinfo(State(s): State<Server>, req: axum::extract::Request) -> Response {
        let headers = req.headers().clone();
        let user = current_user(&s.store, &headers);
        match user {
            Some(u) => json_response(
                StatusCode::OK,
                json!({
                    "sub": u.id,
                    "email": u.email,
                    "email_verified": true,
                    "name": u.display_name,
                }),
            ),
            None => write_err(StatusCode::UNAUTHORIZED, "not authenticated"),
        }
    }

    async fn handlers_authorize(
        State(s): State<Server>,
        Query(params): Query<HashMap<String, String>>,
        headers: HeaderMap,
        req: axum::extract::Request,
    ) -> Response {
        let has_oidc = params.contains_key("response_type")
            || params.contains_key("client_id")
            || params.contains_key("code_challenge");
        if !has_oidc {
            let path = req.uri().path().to_string();
            if path == "/authorize" {
                if let Some(code) = params.get("code")
                    && !code.is_empty()
                {
                    return serve_page(crate::assets::AUTHORIZE_HTML);
                }
                return serve_page(crate::assets::AUTHORIZE_HTML);
            }
            return StatusCode::NOT_FOUND.into_response();
        }
        let client_id = params.get("client_id").cloned().unwrap_or_default();
        let redirect_uri = params.get("redirect_uri").cloned().unwrap_or_default();
        let state = params.get("state").cloned().unwrap_or_default();
        let code_challenge = params.get("code_challenge").cloned().unwrap_or_default();
        let code_challenge_method = params
            .get("code_challenge_method")
            .cloned()
            .unwrap_or_else(|| "plain".to_string());
        let nonce = params.get("nonce").cloned().unwrap_or_default();
        if client_id.is_empty() || redirect_uri.is_empty() {
            return write_err(StatusCode::BAD_REQUEST, "missing client_id or redirect_uri");
        }
        if !Self::is_loopback_redirect(&redirect_uri) {
            return write_err(StatusCode::BAD_REQUEST, "invalid redirect_uri");
        }
        let user = current_user(&s.store, &headers).or_else(|| {
            params
                .get("sid")
                .or_else(|| params.get("token"))
                .and_then(|sid| {
                    s.store
                        .find_session(sid)
                        .and_then(|sess| s.store.find_user_by_id(&sess.user_id))
                })
        });
        let Some(user) = user else {
            let original = req.uri().to_string();
            let next = urlencoding::encode(&original);
            return (
                StatusCode::FOUND,
                [(
                    header::LOCATION,
                    HeaderValue::from_str(&format!("/login?next={next}")).unwrap(),
                )],
            )
                .into_response();
        };
        let code = password::random_hex(16);
        let oidc_code = OidcCode {
            user_id: user.id.clone(),
            client_id: client_id.clone(),
            redirect_uri: redirect_uri.clone(),
            code_challenge: code_challenge.clone(),
            code_challenge_method: code_challenge_method.clone(),
            nonce: nonce.clone(),
            expires_at: Utc::now() + Duration::minutes(10),
        };
        s.oidc_codes.lock().unwrap().insert(code.clone(), oidc_code);
        let mut redirect = format!("{}?code={}", redirect_uri, code);
        if !state.is_empty() {
            redirect.push_str(&format!("&state={}", urlencoding::encode(&state)));
        }
        (
            StatusCode::FOUND,
            [(header::LOCATION, HeaderValue::from_str(&redirect).unwrap())],
        )
            .into_response()
    }

    async fn handlers_token(State(s): State<Server>, req: axum::extract::Request) -> Response {
        let (_parts, body) = req.into_parts();
        let bytes = axum::body::to_bytes(body, 64 << 10)
            .await
            .unwrap_or_default();
        let body_str = String::from_utf8_lossy(&bytes).to_string();
        let params: HashMap<String, String> = url::form_urlencoded::parse(body_str.as_bytes())
            .into_owned()
            .collect();
        let grant_type = params.get("grant_type").cloned().unwrap_or_default();
        if grant_type == "authorization_code" {
            return Self::handle_token_authorization_code(s, params).await;
        } else if grant_type == "refresh_token" {
            return Self::handle_token_refresh(s, params).await;
        } else if grant_type == DEVICE_GRANT_TYPE {
            return write_err(
                StatusCode::BAD_REQUEST,
                "use /api/auth/device/token for device flow",
            );
        }
        write_err(StatusCode::BAD_REQUEST, "unsupported grant_type")
    }

    fn is_loopback_redirect(uri: &str) -> bool {
        if let Ok(u) = url::Url::parse(uri) {
            matches!(
                u.host_str(),
                Some("127.0.0.1") | Some("localhost") | Some("::1")
            ) && u.scheme() == "http"
        } else {
            false
        }
    }

    async fn handle_token_authorization_code(
        s: Server,
        params: HashMap<String, String>,
    ) -> Response {
        let code = params.get("code").cloned().unwrap_or_default();
        let code_verifier = params.get("code_verifier").cloned().unwrap_or_default();
        let client_id = params.get("client_id").cloned().unwrap_or_default();
        let redirect_uri = params.get("redirect_uri").cloned().unwrap_or_default();
        if code.is_empty() || client_id.is_empty() || redirect_uri.is_empty() {
            return write_err(
                StatusCode::BAD_REQUEST,
                "missing code/client_id/redirect_uri",
            );
        }
        let stored = {
            let mut map = s.oidc_codes.lock().unwrap();
            match map.remove(&code) {
                Some(c) => c,
                None => return write_err(StatusCode::BAD_REQUEST, "invalid or expired code"),
            }
        };
        if stored.expires_at < Utc::now() {
            return write_err(StatusCode::BAD_REQUEST, "code expired");
        }
        if stored.client_id != client_id || stored.redirect_uri != redirect_uri {
            return write_err(
                StatusCode::BAD_REQUEST,
                "client_id or redirect_uri mismatch",
            );
        }
        if !stored.code_challenge.is_empty() {
            let computed = if stored.code_challenge_method == "S256" {
                let hash = Sha256::digest(code_verifier.as_bytes());
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
            } else {
                code_verifier.clone()
            };
            if computed != stored.code_challenge {
                return write_err(StatusCode::BAD_REQUEST, "code_verifier mismatch");
            }
        }
        let user = match s.store.find_user_by_id(&stored.user_id) {
            Some(u) => u,
            None => return write_err(StatusCode::BAD_REQUEST, "user not found"),
        };
        let base = s.opts.base_url.trim_end_matches('/').to_string();
        let base = if base.is_empty() {
            "https://astracode.topodrive.top".to_string()
        } else {
            base
        };
        let now = Utc::now();
        let exp = now + Duration::days(30);
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        let claims = serde_json::json!({
            "sub": user.id,
            "iss": base,
            "aud": client_id,
            "exp": exp.timestamp(),
            "iat": now.timestamp(),
            "email": user.email,
            "principal_type": "Team",
            "principal_id": user.id,
            "nonce": stored.nonce,
        });
        let access_token = match jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(s.oidc_jwt_secret.as_bytes()),
        ) {
            Ok(t) => t,
            Err(_) => {
                return write_err(StatusCode::INTERNAL_SERVER_ERROR, "token generation failed");
            }
        };
        let refresh_token = password::random_hex(24);
        let _ = s.store.create_token(&ApiToken {
            token: access_token.clone(),
            user_id: user.id.clone(),
            label: Some("oidc".to_string()),
            created_at: now,
            expires_at: exp,
        });
        let _ = s.store.create_token(&ApiToken {
            token: refresh_token.clone(),
            user_id: user.id.clone(),
            label: Some("refresh".to_string()),
            created_at: now,
            expires_at: now + Duration::days(60),
        });
        json_response(
            StatusCode::OK,
            json!({
                "access_token": access_token,
                "refresh_token": refresh_token,
                "token_type": "Bearer",
                "expires_in": 30*24*3600,
            }),
        )
    }

    async fn handle_token_refresh(s: Server, params: HashMap<String, String>) -> Response {
        let refresh_token = params.get("refresh_token").cloned().unwrap_or_default();
        let client_id = params.get("client_id").cloned().unwrap_or_default();
        if refresh_token.is_empty() {
            return write_err(StatusCode::BAD_REQUEST, "missing refresh_token");
        }
        let tok = match s.store.find_token(&refresh_token) {
            Some(t) => t,
            None => return write_err(StatusCode::BAD_REQUEST, "invalid refresh_token"),
        };
        let user = match s.store.find_user_by_id(&tok.user_id) {
            Some(u) => u,
            None => return write_err(StatusCode::BAD_REQUEST, "user not found"),
        };
        let base = s.opts.base_url.trim_end_matches('/').to_string();
        let base = if base.is_empty() {
            "https://astracode.topodrive.top".to_string()
        } else {
            base
        };
        let now = Utc::now();
        let exp = now + Duration::days(30);
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        let claims = serde_json::json!({
            "sub": user.id,
            "iss": base,
            "aud": if client_id.is_empty() { "unknown".to_string() } else { client_id },
            "exp": exp.timestamp(),
            "iat": now.timestamp(),
            "email": user.email,
            "principal_type": "Team",
            "principal_id": user.id,
        });
        let access_token = match jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(s.oidc_jwt_secret.as_bytes()),
        ) {
            Ok(t) => t,
            Err(_) => {
                return write_err(StatusCode::INTERNAL_SERVER_ERROR, "token generation failed");
            }
        };
        let _ = s.store.create_token(&ApiToken {
            token: access_token.clone(),
            user_id: user.id.clone(),
            label: Some("oidc".to_string()),
            created_at: now,
            expires_at: exp,
        });
        json_response(
            StatusCode::OK,
            json!({
                "access_token": access_token,
                "token_type": "Bearer",
                "expires_in": 30*24*3600,
            }),
        )
    }

    /// Serve the embedded static site pages. Unknown paths 404.
    async fn static_site(State(state): State<Server>, req: axum::extract::Request) -> Response {
        let path = req.uri().path().to_string();

        // /assets/* served from the embedded assets directory.
        if let Some(rest) = path.strip_prefix("/assets/") {
            let content = match rest {
                "site.css" => crate::assets::SITE_CSS,
                "auth.js" => crate::assets::AUTH_JS,
                "admin.js" => crate::assets::ADMIN_JS,
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
            "/login" => {
                if current_user(&state.store, req.headers()).is_some() {
                    redirect_to(login_destination(req.uri().query()))
                } else {
                    serve_page(crate::assets::LOGIN_HTML)
                }
            }
            "/authorize" => serve_page(crate::assets::AUTHORIZE_HTML),
            "/account" => serve_page(crate::assets::ACCOUNT_HTML),
            "/admin" | "/admin.html" => match current_user(&state.store, req.headers()) {
                Some(user) if user.is_admin => redirect_to("/account#admin"),
                Some(_) => redirect_to("/account"),
                None => redirect_to("/login?next=%2Faccount%23admin"),
            },
            "/favicon.svg" => (
                [(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("image/svg+xml"),
                )],
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
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
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

fn redirect_to(location: &str) -> Response {
    match HeaderValue::from_str(location) {
        Ok(value) => (StatusCode::SEE_OTHER, [(header::LOCATION, value)]).into_response(),
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    }
}

/// Only allow same-site destinations that are used by the account flow.
/// This prevents `/login?next=` from becoming an open redirect.
fn login_destination(query: Option<&str>) -> &'static str {
    let next = query.and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "next")
            .map(|(_, value)| value.into_owned())
    });
    match next.as_deref() {
        Some("/account#admin") => "/account#admin",
        Some("/authorize") => "/authorize",
        _ => "/account",
    }
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
        return Err(write_err(
            StatusCode::FORBIDDEN,
            "cross-site request rejected",
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|_| write_err(StatusCode::BAD_REQUEST, "invalid JSON body"))
}

/// Collect every session credential supplied by the browser. Some browsers
/// send more than one same-name cookie when an older cookie was set on a
/// narrower path. Keep checking the bearer and X-Session-Token fallbacks if
/// an earlier cookie is stale or no longer exists in the session store.
fn session_token_candidates(headers: &HeaderMap) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut push = |candidate: &str| {
        let candidate = candidate.trim();
        if !candidate.is_empty() && !tokens.iter().any(|existing| existing.as_str() == candidate) {
            tokens.push(candidate.to_string());
        }
    };

    for cookie_header in headers.get_all(header::COOKIE).iter() {
        if let Ok(cookie_header) = cookie_header.to_str() {
            for cookie in cookie_header.split(';') {
                if let Some(value) = cookie.trim().strip_prefix("sid=") {
                    push(value);
                }
            }
        }
    }
    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    {
        push(value);
    }
    if let Some(value) = headers
        .get("x-session-token")
        .and_then(|value| value.to_str().ok())
    {
        push(value);
    }
    tokens
}

/// Find a valid session among all cookies and fallback session headers.
fn session_for_headers(store: &Store, headers: &HeaderMap) -> Option<(String, User)> {
    session_token_candidates(headers)
        .into_iter()
        .find_map(|token| {
            let session = store.find_session(&token)?;
            let user = store.find_user_by_id(&session.user_id)?;
            Some((token, user))
        })
}

fn session_user(store: &Store, headers: &HeaderMap) -> Option<User> {
    session_for_headers(store, headers).map(|(_, user)| user)
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
        "is_admin": u.is_admin,
        "balance_microusd": u.balance_microusd,
    });
    if let Some(name) = &u.display_name
        && !name.is_empty()
    {
        v["display_name"] = json!(name);
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

fn create_session_token(store: &Store, user_id: &str) -> Result<String, Response> {
    let token = password::random_hex(24);
    let expires = Utc::now() + Duration::days(SESSION_TTL_DAYS);
    store
        .create_session(&Session {
            token: token.clone(),
            user_id: user_id.to_string(),
            expires_at: expires,
        })
        .map_err(|_| write_err(StatusCode::INTERNAL_SERVER_ERROR, "session failed"))?;
    Ok(token)
}

fn set_session_cookie(
    store: &Store,
    opts: &Options,
    user_id: &str,
) -> Result<(String, HeaderValue), Response> {
    let token = create_session_token(store, user_id)?;
    let secure = if opts.cookie_secure { "; Secure" } else { "" };
    let value = format!(
        "sid={token}; Path={}; HttpOnly; SameSite=Lax{secure}; Max-Age={}",
        opts.cookie_path,
        SESSION_TTL_DAYS * 24 * 3600
    );
    let hv = HeaderValue::from_str(&value)
        .map_err(|_| write_err(StatusCode::INTERNAL_SERVER_ERROR, "session failed"))?;
    Ok((token, hv))
}

fn clear_session_cookie(opts: &Options) -> HeaderValue {
    let secure = if opts.cookie_secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "sid=; Path={}; HttpOnly; SameSite=Lax{secure}; Max-Age=-1",
        opts.cookie_path,
    ))
    .unwrap()
}

// --- registration ---

async fn handlers_register(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    #[derive(Deserialize)]
    struct Body {
        email: String,
        password: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]).await {
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
    let stored = s.store.upsert_pending(&PendingRegistration {
        email: email.clone(),
        password_hash: hash,
        token: token.clone(),
        expires_at: now + Duration::hours(24),
    });
    if stored.is_err() {
        return write_err(StatusCode::INTERNAL_SERVER_ERROR, "storage failed");
    }
    let url = verify_url(&s.opts.base_url, &token);
    if let Err(e) = s.mailer.send_verification(&email, &url) {
        tracing::warn!(email = %email, error = %e, "verification email failed");
    }
    json_response(StatusCode::OK, json!({ "ok": true }))
}

async fn handlers_resend(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    #[derive(Deserialize)]
    struct Body {
        email: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let email = crate::store::normalize_email(&body.email);
    if let Some(p) = s.store.find_pending(&email)
        && p.expires_at > Utc::now()
    {
        let url = verify_url(&s.opts.base_url, &p.token);
        let _ = s.mailer.send_verification(&email, &url);
    }
    json_response(StatusCode::OK, json!({ "ok": true }))
}

#[derive(Deserialize)]
struct VerifyQuery {
    token: String,
}

async fn handlers_verify(
    State(s): State<Server>,
    Query(q): Query<VerifyQuery>,
    headers: HeaderMap,
) -> Response {
    let token = &q.token;
    let p = match s.store.find_pending_by_token(token) {
        Some(p) => p,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "Verification link is invalid or expired. Please register again.",
            )
                .into_response();
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
                is_admin: !s.admin_email.is_empty() && p.email.eq_ignore_ascii_case(&s.admin_email),
                balance_microusd: 0,
            };
            let created = s.store.create_user(&u);
            if created.is_err() {
                return write_err(StatusCode::INTERNAL_SERVER_ERROR, "storage failed");
            }
            u
        }
    };
    let _ = s.store.delete_pending(&p.email);
    let (sid, cookie) = match set_session_cookie(&s.store, &s.opts, &user.id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let wants_html = headers
        .get(header::ACCEPT)
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .map(|a: &str| a.contains("text/html"))
        .unwrap_or(false);
    if wants_html {
        // The interstitial is styled in-page rather than served from the
        // embedded site bundle: it is a one-shot redirect, and keeping it
        // self-contained avoids a second round-trip for the CSS.
        let html = VERIFY_HTML.replace("__SID__", &sid);
        return (
            StatusCode::OK,
            [
                (header::SET_COOKIE, cookie),
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
            Body::from(html),
        )
            .into_response();
    }
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, HeaderValue::from_static("/account")),
            (header::SET_COOKIE, cookie),
            (
                axum::http::HeaderName::from_static("x-session-token"),
                HeaderValue::from_str(&sid).unwrap(),
            ),
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
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    #[derive(Deserialize)]
    struct Body {
        email: String,
        password: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(mut user) = s.store.find_user_by_email(&body.email) else {
        return write_err(StatusCode::UNAUTHORIZED, "invalid email or password");
    };
    if !password::check_password(&user.password_hash, &body.password) {
        return write_err(StatusCode::UNAUTHORIZED, "invalid email or password");
    }
    if !s.admin_email.is_empty() && user.email.eq_ignore_ascii_case(&s.admin_email) {
        let _ = s.store.promote_admin_by_email(&s.admin_email);
        user = s.store.find_user_by_id(&user.id).unwrap_or(user);
    }
    let (sid, cookie) = match set_session_cookie(&s.store, &s.opts, &user.id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    // Cache-Control: no-store prevents Edge's bfcache (and other browsers'
    // similar caches) from replaying a stale `/login` JSON response after the
    // cookie or localStorage token has been cleared. Without it, a
    // sign-out → sign-in → back-button sequence can leave `/login` showing a
    // stale user object, which in turn redirects to `/account`, which finds
    // no user, which redirects back to `/login` — the loop the user
    // reported on Edge Strict tracking-prevention mode. The Vary header
    // makes caches key on Cookie / Authorization so different sessions
    // never share a stored body.
    (
        StatusCode::OK,
        [
            (header::SET_COOKIE, cookie),
            (
                axum::http::HeaderName::from_static("x-session-token"),
                HeaderValue::from_str(&sid).unwrap(),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::VARY,
                HeaderValue::from_static("Cookie, Authorization, X-Session-Token"),
            ),
        ],
        Json(json!({ "user": to_public(&user), "token": sid })),
    )
        .into_response()
}

async fn handlers_logout(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let headers = req.headers().clone();
    if let Some((sid, _)) = session_for_headers(&s.store, &headers) {
        let _ = s.store.delete_session(&sid);
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
    let body = match user {
        Some(u) => json!({ "user": to_public(&u) }),
        // 200 + user:null instead of 401 (the login/account pages rely on it).
        None => json!({ "user": null }),
    };
    // Cache-Control: no-store + Vary mirrors the /login response so
    // shared HTTP caches cannot serve a stale `/me` body to a different
    // session after a sign-in/sign-out round trip (the same bfcache
    // redirect-loop class of bug the /login headers prevent).
    (
        StatusCode::OK,
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::VARY,
                HeaderValue::from_static("Cookie, Authorization, X-Session-Token"),
            ),
        ],
        Json(body),
    )
        .into_response()
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
    let stored = s.store.create_device(&g);
    if stored.is_err() {
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

async fn handlers_device_approve(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let user = current_user(&s.store, &parts.headers);
    let Some(user) = user else {
        return write_err(StatusCode::UNAUTHORIZED, "please log in first");
    };
    #[derive(Deserialize)]
    struct Body {
        #[serde(rename = "user_code")]
        user_code: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]).await {
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
    let stored = s.store.create_token(&token);
    if stored.is_err() {
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

async fn handlers_device_token(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    #[derive(Deserialize)]
    struct Body {
        #[serde(rename = "device_code")]
        device_code: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]).await {
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
            let Some(user) = g
                .user_id
                .as_ref()
                .and_then(|id| s.store.find_user_by_id(id))
            else {
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

/// Reverse flow: authenticated user generates a one-time code on the device/browser;
/// CLI consumes it by POSTing the `user_code`. Separated from the RFC8628 forward flow.
async fn handlers_device_generate(
    State(s): State<Server>,
    req: axum::extract::Request,
) -> Response {
    let user = current_user(&s.store, req.headers());
    let Some(user) = user else {
        return write_err(StatusCode::UNAUTHORIZED, "please log in first");
    };
    let g = DeviceGrant {
        device_code: password::random_hex(16),
        user_code: password::random_user_code(),
        status: "pending".to_string(),
        user_id: Some(user.id.clone()),
        token: None,
        created_at: Utc::now(),
        expires_at: Utc::now() + Duration::seconds(DEVICE_EXPIRY_SECS),
        approved_at: None,
    };
    if s.store.create_device(&g).is_err() {
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

async fn handlers_device_consume(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    #[derive(Deserialize)]
    struct Body {
        #[serde(rename = "user_code")]
        user_code: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let code = body.user_code.trim().to_uppercase();
    let Some(g) = s.store.find_device_by_user_code(&code) else {
        return write_err(StatusCode::BAD_REQUEST, "invalid or expired code");
    };
    if g.status != "pending" || g.expires_at < Utc::now() {
        return write_err(StatusCode::BAD_REQUEST, "invalid or expired code");
    }
    // Reverse flow grants are pre-bound to a user; forward-flow grants (user_id==None) are rejected here.
    let Some(user_id) = g.user_id.clone() else {
        return write_err(StatusCode::BAD_REQUEST, "invalid or expired code");
    };
    let Some(user) = s.store.find_user_by_id(&user_id) else {
        return write_err(StatusCode::BAD_REQUEST, "invalid or expired code");
    };
    let token = ApiToken {
        token: password::random_hex(24),
        user_id: user.id.clone(),
        label: Some("device".to_string()),
        created_at: Utc::now(),
        expires_at: Utc::now() + Duration::days(TOKEN_TTL_DAYS),
    };
    if s.store.create_token(&token).is_err() {
        return write_err(StatusCode::INTERNAL_SERVER_ERROR, "storage failed");
    }
    let now = Utc::now();
    let token_str = token.token.clone();
    let _ = s.store.update_device(&g.device_code, |d| {
        d.status = "approved".to_string();
        d.token = Some(token_str.clone());
        d.approved_at = Some(now);
    });
    json_response(
        StatusCode::OK,
        json!({
            "status": "approved",
            "access_token": token.token,
            "user": to_public(&user),
        }),
    )
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
    let stored = s.store.create_token(&tok);
    if stored.is_err() {
        return write_err(StatusCode::INTERNAL_SERVER_ERROR, "storage failed");
    }
    json_response(StatusCode::OK, json!({ "token": tok.token }))
}

async fn handlers_tokens_delete(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let Some(_user) = current_user(&s.store, &parts.headers) else {
        return write_err(StatusCode::UNAUTHORIZED, "not authenticated");
    };
    #[derive(Deserialize)]
    struct Body {
        token: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]).await {
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
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let Some(mut user) = current_user(&s.store, &parts.headers) else {
        return write_err(StatusCode::UNAUTHORIZED, "not authenticated");
    };
    #[derive(Deserialize)]
    struct Body {
        #[serde(rename = "display_name")]
        display_name: String,
    }
    let body: Body = match decode_body(&parts.method, &parts.headers, &host, &bytes[..]).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let name = body.display_name.trim();
    let name: String = name.chars().take(60).collect();
    let user_id = user.id.clone();
    let updated = s.store.update_user(&user_id, |u| {
        u.display_name = Some(name.clone());
    });
    if updated.is_err() {
        return write_err(StatusCode::INTERNAL_SERVER_ERROR, "update failed");
    }
    user.display_name = Some(name);
    json_response(StatusCode::OK, json!({ "user": to_public(&user) }))
}

// --- balance and model routing ---

fn admin_user(s: &Server, headers: &HeaderMap) -> Result<User, Response> {
    match current_user(&s.store, headers) {
        Some(user) if user.is_admin => Ok(user),
        Some(_) => Err(write_err(
            StatusCode::FORBIDDEN,
            "administrator access required",
        )),
        None => Err(write_err(StatusCode::UNAUTHORIZED, "not authenticated")),
    }
}

fn route_public(route: &ModelRoute) -> Value {
    json!({
        "id": route.id,
        "display_name": route.display_name,
        "protocol": route.protocol,
        "base_url": route.base_url,
        "upstream_model": route.upstream_model,
        "context_window": route.context_window,
        "api_key_set": route.api_key.as_deref().is_some_and(|key| !key.is_empty()),
        "input_price_usd_per_million": route.input_price_microusd_per_million as f64 / 1_000_000.0,
        "output_price_usd_per_million": route.output_price_microusd_per_million as f64 / 1_000_000.0,
        "enabled": route.enabled,
        "is_default": route.is_default,
    })
}

fn amount_usd_to_microusd(amount: f64) -> Option<i64> {
    if !amount.is_finite() || amount < 0.0 || amount > 1_000_000_000.0 {
        return None;
    }
    Some((amount * 1_000_000.0).round() as i64)
}

fn model_route_id(display_name: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for ch in display_name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !slug.is_empty() {
            slug.push('-');
            dash = true;
        }
    }
    slug.trim_matches('-').chars().take(64).collect()
}

fn valid_upstream_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() {
        return false;
    }
    match url.scheme() {
        "https" => true,
        "http" => matches!(url.host_str(), Some("localhost") | Some("127.0.0.1")),
        _ => false,
    }
}

fn enabled_by_default() -> bool {
    true
}

async fn handlers_balance(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let headers = req.headers().clone();
    let Some(user) = current_user(&s.store, &headers) else {
        return write_err(StatusCode::UNAUTHORIZED, "not authenticated");
    };
    let transactions: Vec<Value> = s
        .store
        .ledger_for_user(&user.id, 30)
        .iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "delta_microusd": entry.delta_microusd,
                "note": entry.note,
                "model": entry.model,
                "input_tokens": entry.input_tokens,
                "output_tokens": entry.output_tokens,
                "created_at": entry.created_at.to_rfc3339(),
            })
        })
        .collect();
    json_response(
        StatusCode::OK,
        json!({ "balance_microusd": user.balance_microusd, "transactions": transactions }),
    )
}

async fn handlers_admin_models_get(
    State(s): State<Server>,
    req: axum::extract::Request,
) -> Response {
    if let Err(response) = admin_user(&s, req.headers()) {
        return response;
    }
    let routes: Vec<Value> = s.store.model_routes().iter().map(route_public).collect();
    json_response(StatusCode::OK, json!({ "routes": routes }))
}

async fn handlers_admin_models_post(
    State(s): State<Server>,
    req: axum::extract::Request,
) -> Response {
    let (parts, body) = req.into_parts();
    if let Err(response) = admin_user(&s, &parts.headers) {
        return response;
    }
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    #[derive(Deserialize)]
    struct RouteBody {
        id: Option<String>,
        display_name: String,
        protocol: String,
        base_url: String,
        upstream_model: String,
        context_window: u64,
        #[serde(default)]
        api_key: Option<String>,
        #[serde(default)]
        clear_api_key: bool,
        input_price_usd_per_million: f64,
        output_price_usd_per_million: f64,
        #[serde(default = "enabled_by_default")]
        enabled: bool,
        #[serde(default)]
        is_default: bool,
    }
    let body: RouteBody = match decode_body(&parts.method, &parts.headers, &host, &bytes).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let display_name = body.display_name.trim();
    let base_url = body.base_url.trim().trim_end_matches('/');
    let upstream_model = body.upstream_model.trim();
    if display_name.is_empty()
        || display_name.chars().count() > 80
        || upstream_model.is_empty()
        || upstream_model.chars().count() > 200
        || !(1_024..=2_000_000).contains(&body.context_window)
        || !valid_upstream_url(base_url)
    {
        return write_err(
            StatusCode::BAD_REQUEST,
            "enter a display name, model id, and valid HTTPS API base URL",
        );
    }
    if !matches!(
        body.protocol.as_str(),
        "openai-compatible" | "anthropic-messages"
    ) {
        return write_err(StatusCode::BAD_REQUEST, "unsupported provider protocol");
    }
    let Some(input_price) = amount_usd_to_microusd(body.input_price_usd_per_million) else {
        return write_err(StatusCode::BAD_REQUEST, "invalid input token price");
    };
    let Some(output_price) = amount_usd_to_microusd(body.output_price_usd_per_million) else {
        return write_err(StatusCode::BAD_REQUEST, "invalid output token price");
    };
    let id = body
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| model_route_id(display_name));
    if id.is_empty() || id.chars().count() > 64 {
        return write_err(StatusCode::BAD_REQUEST, "invalid CLI model id");
    }
    let existing = s.store.model_route(&id);
    let api_key = if body.clear_api_key {
        None
    } else if let Some(key) = body.api_key.filter(|key| !key.trim().is_empty()) {
        Some(key.trim().to_string())
    } else {
        existing.and_then(|route| route.api_key)
    };
    let route = ModelRoute {
        id,
        display_name: display_name.to_string(),
        protocol: body.protocol,
        base_url: base_url.to_string(),
        upstream_model: upstream_model.to_string(),
        context_window: body.context_window,
        api_key,
        input_price_microusd_per_million: input_price as u64,
        output_price_microusd_per_million: output_price as u64,
        enabled: body.enabled,
        is_default: body.is_default,
    };
    if s.store.upsert_model_route(route).is_err() {
        return write_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not save model route",
        );
    }
    let routes: Vec<Value> = s.store.model_routes().iter().map(route_public).collect();
    json_response(StatusCode::OK, json!({ "routes": routes }))
}

async fn handlers_admin_models_delete(
    State(s): State<Server>,
    req: axum::extract::Request,
) -> Response {
    let (parts, body) = req.into_parts();
    if let Err(response) = admin_user(&s, &parts.headers) {
        return response;
    }
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    #[derive(Deserialize)]
    struct DeleteBody {
        id: String,
    }
    let body: DeleteBody = match decode_body(&parts.method, &parts.headers, &host, &bytes).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    match s.store.delete_model_route(&body.id) {
        Ok(true) => json_response(StatusCode::OK, json!({ "ok": true })),
        Ok(false) => write_err(StatusCode::NOT_FOUND, "model route not found"),
        Err(_) => write_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not delete model route",
        ),
    }
}

async fn handlers_admin_users(State(s): State<Server>, req: axum::extract::Request) -> Response {
    if let Err(response) = admin_user(&s, req.headers()) {
        return response;
    }
    let users: Vec<Value> = s
        .store
        .users()
        .iter()
        .map(|user| {
            json!({
                "id": user.id,
                "email": user.email,
                "display_name": user.display_name,
                "is_admin": user.is_admin,
                "balance_microusd": user.balance_microusd,
                "created_at": user.created_at.to_rfc3339(),
            })
        })
        .collect();
    json_response(StatusCode::OK, json!({ "users": users }))
}

async fn handlers_admin_balance(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    if let Err(response) = admin_user(&s, &parts.headers) {
        return response;
    }
    let bytes = axum::body::to_bytes(body, 64 << 10)
        .await
        .unwrap_or_default();
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    #[derive(Deserialize)]
    struct CreditBody {
        user_id: String,
        amount_usd: f64,
        note: Option<String>,
    }
    let body: CreditBody = match decode_body(&parts.method, &parts.headers, &host, &bytes).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(amount) = amount_usd_to_microusd(body.amount_usd) else {
        return write_err(StatusCode::BAD_REQUEST, "enter a valid credit amount");
    };
    if amount == 0 {
        return write_err(
            StatusCode::BAD_REQUEST,
            "credit amount must be greater than zero",
        );
    }
    if s.store
        .adjust_balance(
            &body.user_id,
            amount,
            body.note
                .as_deref()
                .map(str::trim)
                .filter(|note| !note.is_empty())
                .unwrap_or("Administrator credit")
                .to_string(),
            None,
            0,
            0,
        )
        .is_err()
    {
        return write_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not update balance",
        );
    }
    let Some(user) = s.store.find_user_by_id(&body.user_id) else {
        return write_err(StatusCode::NOT_FOUND, "user not found");
    };
    json_response(
        StatusCode::OK,
        json!({ "balance_microusd": user.balance_microusd }),
    )
}

#[derive(Default, Clone, Copy)]
struct TokenUsage {
    input: u64,
    output: u64,
}

impl TokenUsage {
    fn read(&mut self, value: &Value) {
        let Some(usage) = value.get("usage") else {
            return;
        };
        self.input = usage
            .get("prompt_tokens")
            .or_else(|| usage.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(self.input);
        self.output = usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(self.output);
    }
}

fn usage_cost_microusd(route: &ModelRoute, usage: TokenUsage) -> i64 {
    let input = u128::from(route.input_price_microusd_per_million)
        .saturating_mul(u128::from(usage.input))
        .saturating_add(999_999)
        / 1_000_000;
    let output = u128::from(route.output_price_microusd_per_million)
        .saturating_mul(u128::from(usage.output))
        .saturating_add(999_999)
        / 1_000_000;
    input.saturating_add(output).min(i64::MAX as u128) as i64
}

fn record_usage(store: &Store, user_id: &str, route: &ModelRoute, usage: TokenUsage) {
    if usage.input == 0 && usage.output == 0 {
        return;
    }
    let cost = usage_cost_microusd(route, usage);
    let _ = store.adjust_balance(
        user_id,
        -cost,
        format!("{} usage", route.display_name),
        Some(route.display_name.clone()),
        usage.input,
        usage.output,
    );
}

fn choose_model_route(store: &Store, requested_model: &str) -> Option<ModelRoute> {
    let routes = store.model_routes();
    let enabled: Vec<_> = routes.into_iter().filter(|route| route.enabled).collect();
    enabled
        .iter()
        .find(|route| {
            route.id == requested_model
                || route.display_name.eq_ignore_ascii_case(requested_model)
                || route.upstream_model == requested_model
        })
        .cloned()
        .or_else(|| enabled.iter().find(|route| route.is_default).cloned())
        .or_else(|| (enabled.len() == 1).then(|| enabled[0].clone()))
}

async fn handlers_gateway_models(State(s): State<Server>, req: axum::extract::Request) -> Response {
    if bearer_user(&s.store, req.headers()).is_none() {
        return write_err(StatusCode::UNAUTHORIZED, "valid Astra CLI token required");
    }
    let models: Vec<Value> = s
        .store
        .model_routes()
        .into_iter()
        .filter(|route| route.enabled)
        .map(|route| {
            json!({
                "id": route.id,
                "object": "model",
                "created": 0,
                "owned_by": "astra",
                "model": route.id,
                "name": route.display_name,
                "display_name": route.display_name,
                "apiBackend": "chat_completions",
                "contextWindow": route.context_window,
            })
        })
        .collect();
    let mut response = json_response(StatusCode::OK, json!({ "object": "list", "data": models }));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::VARY,
        HeaderValue::from_static("Authorization, Cookie, X-Session-Token"),
    );
    response
}

async fn handlers_gateway_chat(State(s): State<Server>, req: axum::extract::Request) -> Response {
    let (parts, body) = req.into_parts();
    let Some(user) = bearer_user(&s.store, &parts.headers) else {
        return write_err(StatusCode::UNAUTHORIZED, "valid Astra CLI token required");
    };
    if user.balance_microusd <= 0 {
        return write_err(StatusCode::PAYMENT_REQUIRED, "account balance is empty");
    }
    let bytes = match axum::body::to_bytes(body, 32 << 20).await {
        Ok(bytes) => bytes,
        Err(_) => return write_err(StatusCode::BAD_REQUEST, "request body is too large"),
    };
    let mut request: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return write_err(StatusCode::BAD_REQUEST, "invalid JSON body"),
    };
    if !request.is_object() {
        return write_err(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
        );
    }
    let requested_model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let Some(route) = choose_model_route(&s.store, &requested_model) else {
        return write_err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no model route is configured",
        );
    };
    if route.protocol == "openai-compatible" {
        request["model"] = json!(route.upstream_model);
        if request.get("stream").and_then(Value::as_bool) == Some(true) {
            let stream_options = request
                .as_object_mut()
                .expect("JSON object")
                .entry("stream_options")
                .or_insert_with(|| json!({}));
            if let Some(options) = stream_options.as_object_mut() {
                options.insert("include_usage".to_string(), json!(true));
            }
        }
    } else {
        request = anthropic_request(&request, &route);
    }
    let endpoint = provider_endpoint(&route);
    let mut upstream_request = s
        .http
        .post(endpoint)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "text/event-stream, application/json")
        .json(&request);
    match route.protocol.as_str() {
        "openai-compatible" => {
            if let Some(key) = route.api_key.as_deref().filter(|key| !key.is_empty()) {
                upstream_request = upstream_request.bearer_auth(key);
            }
        }
        "anthropic-messages" => {
            if let Some(key) = route.api_key.as_deref().filter(|key| !key.is_empty()) {
                upstream_request = upstream_request.header("x-api-key", key);
            }
            upstream_request = upstream_request.header("anthropic-version", "2023-06-01");
        }
        _ => return write_err(StatusCode::BAD_REQUEST, "unsupported provider protocol"),
    }
    let upstream = match upstream_request.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(model = %route.display_name, error = %error, "upstream model request failed");
            return write_err(StatusCode::BAD_GATEWAY, "upstream provider is unavailable");
        }
    };
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    if status.is_success()
        && content_type
            .to_ascii_lowercase()
            .contains("text/event-stream")
    {
        return gateway_stream_response(s, upstream, status, &content_type, user.id, route);
    }
    let bytes = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => return write_err(StatusCode::BAD_GATEWAY, "could not read provider response"),
    };
    let mut response_body = bytes.to_vec();
    if status.is_success()
        && let Ok(mut value) = serde_json::from_slice::<Value>(&bytes)
    {
        let mut usage = TokenUsage::default();
        usage.read(&value);
        if route.protocol == "anthropic-messages" {
            value = anthropic_response(value, &route.id, usage);
        } else if let Some(object) = value.as_object_mut() {
            object.insert("model".to_string(), json!(route.id));
        }
        response_body = serde_json::to_vec(&value).unwrap_or(response_body);
        record_usage(&s.store, &user.id, &route, usage);
    }
    provider_response(status, &content_type, response_body)
}

fn provider_endpoint(route: &ModelRoute) -> String {
    let suffix = if route.protocol == "anthropic-messages" {
        "/messages"
    } else {
        "/chat/completions"
    };
    format!("{}{suffix}", route.base_url.trim_end_matches('/'))
}

fn anthropic_request(request: &Value, route: &ModelRoute) -> Value {
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut system = Vec::<String>::new();
    let mut output = Vec::<Value>::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        if role == "system" || role == "developer" {
            if let Some(text) = content.as_str() {
                system.push(text.to_string());
            }
            continue;
        }
        if role == "tool" {
            output.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": message.get("tool_call_id").cloned().unwrap_or(Value::Null),
                    "content": content,
                }],
            }));
            continue;
        }
        let mut blocks = Vec::<Value>::new();
        if let Some(text) = content.as_str().filter(|text| !text.is_empty()) {
            blocks.push(json!({ "type": "text", "text": text }));
        } else if let Some(parts) = content.as_array() {
            for part in parts {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => blocks.push(json!({
                        "type": "text",
                        "text": part.get("text").cloned().unwrap_or(json!("")),
                    })),
                    Some("image_url") => {
                        let image_url = part
                            .get("image_url")
                            .and_then(|image| image.get("url"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if let Some(encoded) = image_url.strip_prefix("data:") {
                            if let Some((mime, data)) = encoded.split_once(";base64,") {
                                blocks.push(json!({
                                    "type": "image",
                                    "source": { "type": "base64", "media_type": mime, "data": data },
                                }));
                            }
                        } else if !image_url.is_empty() {
                            blocks.push(json!({
                                "type": "image",
                                "source": { "type": "url", "url": image_url },
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let function = call.get("function").cloned().unwrap_or_else(|| json!({}));
                let raw_input = function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                let input = serde_json::from_str::<Value>(raw_input)
                    .unwrap_or_else(|_| json!({ "_raw": raw_input }));
                blocks.push(json!({
                    "type": "tool_use",
                    "id": call.get("id").cloned().unwrap_or(Value::Null),
                    "name": function.get("name").cloned().unwrap_or(Value::Null),
                    "input": input,
                }));
            }
        }
        if blocks.is_empty() {
            blocks.push(json!({ "type": "text", "text": "" }));
        }
        output.push(json!({
            "role": if role == "assistant" { "assistant" } else { "user" },
            "content": blocks,
        }));
    }
    let mut body = json!({
        "model": route.upstream_model,
        "messages": output,
        "max_tokens": request.get("max_tokens")
            .or_else(|| request.get("max_completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(4096),
        "stream": request.get("stream").and_then(Value::as_bool).unwrap_or(false),
    });
    if !system.is_empty() {
        body["system"] = json!(system.join("\n\n"));
    }
    for key in ["temperature", "top_p", "stop"] {
        if let Some(value) = request.get(key) {
            body[key] = value.clone();
        }
    }
    if let Some(tools) = request.get("tools").and_then(Value::as_array) {
        let mapped: Vec<Value> = tools
            .iter()
            .filter_map(|tool| tool.get("function"))
            .map(|function| {
                json!({
                    "name": function.get("name").cloned().unwrap_or(Value::Null),
                    "description": function.get("description").cloned().unwrap_or(Value::Null),
                    "input_schema": function.get("parameters").cloned().unwrap_or(json!({ "type": "object" })),
                })
            })
            .collect();
        if !mapped.is_empty() {
            body["tools"] = json!(mapped);
        }
    }
    if let Some(choice) = request.get("tool_choice") {
        if choice.as_str() == Some("none") {
            body.as_object_mut().map(|object| object.remove("tools"));
        } else {
            body["tool_choice"] = match choice.as_str() {
                Some("required") => json!({ "type": "any" }),
                Some("auto") => json!({ "type": "auto" }),
                _ => choice
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .map(|name| json!({ "type": "tool", "name": name }))
                    .unwrap_or(Value::Null),
            };
        }
    }
    body
}

fn anthropic_response(value: Value, public_model: &str, usage: TokenUsage) -> Value {
    let mut text = String::new();
    let mut tool_calls = Vec::<Value>::new();
    if let Some(content) = value.get("content").and_then(Value::as_array) {
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    text.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""))
                }
                Some("tool_use") => {
                    let arguments = block.get("input").cloned().unwrap_or(json!({}));
                    tool_calls.push(json!({
                        "id": block.get("id").cloned().unwrap_or(Value::Null),
                        "type": "function",
                        "function": {
                            "name": block.get("name").cloned().unwrap_or(Value::Null),
                            "arguments": serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".into()),
                        }
                    }));
                }
                _ => {}
            }
        }
    }
    let mut message = json!({ "role": "assistant", "content": text });
    if !tool_calls.is_empty() {
        message["tool_calls"] = json!(tool_calls);
    }
    let finish_reason = match value.get("stop_reason").and_then(Value::as_str) {
        Some("tool_use") => "tool_calls",
        Some("max_tokens") => "length",
        _ => "stop",
    };
    json!({
        "id": value.get("id").cloned().unwrap_or(json!("astra")),
        "object": "chat.completion",
        "created": Utc::now().timestamp(),
        "model": public_model,
        "choices": [{ "index": 0, "message": message, "finish_reason": finish_reason }],
        "usage": {
            "prompt_tokens": usage.input,
            "completion_tokens": usage.output,
            "total_tokens": usage.input.saturating_add(usage.output),
        },
    })
}

fn provider_response(status: StatusCode, content_type: &str, bytes: Vec<u8>) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

fn find_sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    if let Some(index) = buffer.windows(2).position(|window| window == b"\n\n") {
        return Some((index, 2));
    }
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4))
}

fn sse_payload(event: &[u8]) -> (String, String) {
    let text = String::from_utf8_lossy(event);
    let mut name = String::new();
    let mut data = String::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            name = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim_start());
        }
    }
    (name, data)
}

fn openai_sse_event(event: &[u8], public_model: &str, usage: &mut TokenUsage) -> Bytes {
    let (_, data) = sse_payload(event);
    if data.is_empty() || data == "[DONE]" {
        return Bytes::copy_from_slice(event);
    }
    let Ok(mut value) = serde_json::from_str::<Value>(&data) else {
        return Bytes::copy_from_slice(event);
    };
    usage.read(&value);
    if let Some(object) = value.as_object_mut() {
        object.insert("model".to_string(), json!(public_model));
    }
    Bytes::from(format!("data: {}\n\n", value))
}

#[derive(Default)]
struct AnthropicStreamState {
    id: String,
    tool_indexes: HashMap<usize, usize>,
    next_tool_index: usize,
    usage: TokenUsage,
}

fn openai_chunk(model: &str, id: &str, delta: Value, finish_reason: Value) -> Value {
    json!({
        "id": if id.is_empty() { "astra" } else { id },
        "object": "chat.completion.chunk",
        "created": 0,
        "model": model,
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }],
    })
}

fn anthropic_sse_event(
    event: &[u8],
    public_model: &str,
    state: &mut AnthropicStreamState,
) -> Vec<Bytes> {
    let (event_name, data) = sse_payload(event);
    let Ok(value) = serde_json::from_str::<Value>(&data) else {
        return Vec::new();
    };
    let output = match event_name.as_str() {
        "message_start" => {
            let message = value.get("message").cloned().unwrap_or_else(|| json!({}));
            state.id = message
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("astra")
                .to_string();
            state.usage.read(&message);
            Some(openai_chunk(
                public_model,
                &state.id,
                json!({ "role": "assistant" }),
                Value::Null,
            ))
        }
        "content_block_start" => {
            let block = value
                .get("content_block")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                let block_index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let tool_index = state.next_tool_index;
                state.next_tool_index += 1;
                state.tool_indexes.insert(block_index, tool_index);
                Some(openai_chunk(
                    public_model,
                    &state.id,
                    json!({
                        "tool_calls": [{
                            "index": tool_index,
                            "id": block.get("id").cloned().unwrap_or(Value::Null),
                            "type": "function",
                            "function": {
                                "name": block.get("name").cloned().unwrap_or(Value::Null),
                                "arguments": "",
                            },
                        }]
                    }),
                    Value::Null,
                ))
            } else {
                None
            }
        }
        "content_block_delta" => {
            let delta = value.get("delta").cloned().unwrap_or_else(|| json!({}));
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => Some(openai_chunk(
                    public_model,
                    &state.id,
                    json!({ "content": delta.get("text").cloned().unwrap_or(json!("")) }),
                    Value::Null,
                )),
                Some("thinking_delta") => Some(openai_chunk(
                    public_model,
                    &state.id,
                    json!({ "reasoning_content": delta.get("thinking").cloned().unwrap_or(json!("")) }),
                    Value::Null,
                )),
                Some("input_json_delta") => {
                    let block_index =
                        value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let tool_index = *state.tool_indexes.get(&block_index).unwrap_or(&block_index);
                    Some(openai_chunk(
                        public_model,
                        &state.id,
                        json!({
                            "tool_calls": [{
                                "index": tool_index,
                                "function": { "arguments": delta.get("partial_json").cloned().unwrap_or(json!("")) },
                            }]
                        }),
                        Value::Null,
                    ))
                }
                _ => None,
            }
        }
        "message_delta" => {
            state.usage.read(&value);
            let stop = value
                .get("delta")
                .and_then(|delta| delta.get("stop_reason"))
                .and_then(Value::as_str)
                .map(|reason| match reason {
                    "tool_use" => "tool_calls",
                    "max_tokens" => "length",
                    _ => "stop",
                });
            let mut chunk = openai_chunk(
                public_model,
                &state.id,
                json!({}),
                stop.map(|reason| json!(reason)).unwrap_or(Value::Null),
            );
            if value.get("usage").is_some() {
                chunk["usage"] = json!({
                    "prompt_tokens": state.usage.input,
                    "completion_tokens": state.usage.output,
                    "total_tokens": state.usage.input.saturating_add(state.usage.output),
                });
            }
            Some(chunk)
        }
        "message_stop" => return vec![Bytes::from_static(b"data: [DONE]\n\n")],
        "error" => Some(json!({
            "error": value.get("error").cloned().unwrap_or(value),
        })),
        _ => None,
    };
    output
        .map(|value| Bytes::from(format!("data: {value}\n\n")))
        .into_iter()
        .collect()
}

fn gateway_stream_response(
    s: Server,
    upstream: reqwest::Response,
    status: StatusCode,
    content_type: &str,
    user_id: String,
    route: ModelRoute,
) -> Response {
    let protocol = route.protocol.clone();
    let store = s.store.clone();
    let output = async_stream::stream! {
        let mut chunks = upstream.bytes_stream();
        let mut buffer = Vec::<u8>::new();
        let mut usage = TokenUsage::default();
        let mut anthropic = AnthropicStreamState::default();
        let mut completed = true;
        let mut charged = false;
        while let Some(chunk) = chunks.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    completed = false;
                    yield Err::<Bytes, std::io::Error>(std::io::Error::other(error));
                    break;
                }
            };
            buffer.extend_from_slice(&chunk);
            while let Some((index, separator)) = find_sse_boundary(&buffer) {
                let end = index + separator;
                let event: Vec<u8> = buffer.drain(..end).collect();
                if protocol == "anthropic-messages" {
                    let (event_name, _) = sse_payload(&event);
                    let parts = anthropic_sse_event(&event, &route.id, &mut anthropic);
                    if status.is_success() && event_name == "message_delta" && !charged {
                        record_usage(&store, &user_id, &route, anthropic.usage);
                        charged = true;
                    }
                    for part in parts {
                        yield Ok::<Bytes, std::io::Error>(part);
                    }
                } else {
                    let is_done = sse_payload(&event).1 == "[DONE]";
                    let part = openai_sse_event(&event, &route.id, &mut usage);
                    if status.is_success() && is_done && !charged {
                        record_usage(&store, &user_id, &route, usage);
                        charged = true;
                    }
                    yield Ok::<Bytes, std::io::Error>(part);
                }
            }
        }
        if completed && !buffer.is_empty() {
            if protocol == "anthropic-messages" {
                let (event_name, _) = sse_payload(&buffer);
                let parts = anthropic_sse_event(&buffer, &route.id, &mut anthropic);
                if status.is_success() && event_name == "message_delta" && !charged {
                    record_usage(&store, &user_id, &route, anthropic.usage);
                    charged = true;
                }
                for part in parts {
                    yield Ok::<Bytes, std::io::Error>(part);
                }
            } else {
                let is_done = sse_payload(&buffer).1 == "[DONE]";
                let part = openai_sse_event(&buffer, &route.id, &mut usage);
                if status.is_success() && is_done && !charged {
                    record_usage(&store, &user_id, &route, usage);
                    charged = true;
                }
                yield Ok::<Bytes, std::io::Error>(part);
            }
        }
        if protocol == "anthropic-messages" {
            usage = anthropic.usage;
        }
        if completed && status.is_success() && !charged {
            record_usage(&store, &user_id, &route, usage);
        }
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(output))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}
