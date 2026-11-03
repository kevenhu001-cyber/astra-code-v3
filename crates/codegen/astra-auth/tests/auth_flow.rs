//! End-to-end HTTP tests mirroring the Go `authsrv_test.go` flows: register →
//! verify → session login, device flow, tokens, account update, CSRF.
//!
//! The server runs on a background thread with its own tokio runtime; the
//! tests drive it with `reqwest::blocking` (a blocking client cannot live
//! inside a tokio runtime, so the tests themselves are plain `#[test]`).
#![allow(clippy::disallowed_methods)] // test clients only call the ephemeral localhost server

use std::sync::Arc;

use astra_auth::mailer::ConsoleMailer;
use astra_auth::server::{Options, Server};
use astra_auth::store::{ApiToken, ModelRoute, Store};
use axum::http::StatusCode;

/// Start a test server on an ephemeral port. Returns (base_url, store); the
/// store is shared with the server so tests observe its mutations. The temp
/// dir lives as long as the server thread.
fn test_server() -> (String, Arc<Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path().join("auth.json")).unwrap());
    let srv = Server::from_arc(
        store.clone(),
        Arc::new(ConsoleMailer),
        Options {
            base_url: "http://test.local".to_string(),
            cookie_secure: false,
            cookie_path: "/".to_string(),
        },
    );
    let app = srv.router();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let _dir = dir; // keep the temp dir alive while the server runs
        rt.block_on(async move {
            let _ = axum::serve(listener, app).await;
        });
    });
    (format!("http://{addr}"), store)
}

fn post_json(url: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let client = reqwest::blocking::Client::new();
    let resp = client.post(url).json(&body).send().unwrap();
    let status = resp.status();
    let json: serde_json::Value = resp.json().unwrap_or(serde_json::Value::Null);
    (status, json)
}

fn seed_user(store: &Store, id: &str, email: &str, password: &str) {
    let hash = astra_auth::password::hash_password(password).unwrap();
    store
        .create_user(&astra_auth::store::User {
            id: id.into(),
            email: email.into(),
            password_hash: hash,
            display_name: None,
            created_at: chrono::Utc::now(),
            verified_at: None,
            is_admin: false,
            balance_microusd: 0,
        })
        .unwrap();
}

#[test]
fn register_verify_login_flow() {
    let (base, store) = test_server();

    // Register → pending, no user yet.
    let (status, out) = post_json(
        &format!("{base}/api/auth/register"),
        serde_json::json!({"email": "User@Example.com ", "password": "password123"}),
    );
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["ok"], true);
    assert!(store.find_user_by_email("user@example.com").is_none());
    let pending = store.find_pending("user@example.com").expect("pending");
    assert_eq!(pending.token.len(), 32);

    // Verify via the link → 302 + user created + session cookie.
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();
    let resp = client
        .get(format!("{base}/api/auth/verify?token={}", pending.token))
        .send()
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert!(store.find_user_by_email("user@example.com").is_some());
    assert!(store.find_pending("user@example.com").is_none());

    // /me with the session cookie.
    let me = client.get(format!("{base}/api/auth/me")).send().unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let body = me.text().unwrap();
    assert!(body.contains("user@example.com"));
}

#[test]
fn login_wrong_password() {
    let (base, store) = test_server();
    seed_user(&store, "u1", "a@b.co", "correct-horse");

    let (status, out) = post_json(
        &format!("{base}/api/auth/login"),
        serde_json::json!({"email": "a@b.co", "password": "wrong"}),
    );
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("invalid email or password")
    );
}

#[test]
fn duplicate_registration_is_silent() {
    let (base, store) = test_server();
    seed_user(&store, "u1", "a@b.co", "password123");

    let (status, out) = post_json(
        &format!("{base}/api/auth/register"),
        serde_json::json!({"email": "a@b.co", "password": "password123"}),
    );
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["ok"], true);
}

#[test]
fn device_flow_end_to_end() {
    let (base, store) = test_server();
    seed_user(&store, "u1", "dev@b.co", "password123");

    // Browser session: log in and keep the cookie.
    let client = reqwest::blocking::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let login = client
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({"email": "dev@b.co", "password": "password123"}))
        .send()
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);

    // 1) CLI requests a device grant.
    let (_, out) = post_json(&format!("{base}/api/auth/device"), serde_json::json!({}));
    let device_code = out["device_code"].as_str().unwrap().to_string();
    let user_code = out["user_code"].as_str().unwrap().to_string();
    // `verification_uri` is the page the user visits; the pre-filled hand-off
    // lives in `verification_uri_complete` so the CLI can show a confirm-only
    // code without putting the code in the address bar twice.
    assert!(
        out["verification_uri"]
            .as_str()
            .unwrap()
            .ends_with("/authorize")
    );
    assert!(
        out["verification_uri_complete"]
            .as_str()
            .unwrap()
            .contains(&user_code)
    );

    // 2) TUI polls → pending.
    let (_, poll) = post_json(
        &format!("{base}/api/auth/device/token"),
        serde_json::json!({"device_code": device_code}),
    );
    assert_eq!(poll["status"], "pending");

    // 3) User approves in the browser (session cookie + Origin header).
    let approve = client
        .post(format!("{base}/api/auth/device/approve"))
        .header("Origin", base.clone())
        .json(&serde_json::json!({"user_code": user_code}))
        .send()
        .unwrap();
    assert_eq!(approve.status(), StatusCode::OK);

    // 4) TUI polls → approved with a bearer token.
    let (_, poll2) = post_json(
        &format!("{base}/api/auth/device/token"),
        serde_json::json!({"device_code": device_code}),
    );
    assert_eq!(poll2["status"], "approved");
    let token = poll2["access_token"].as_str().unwrap().to_string();
    assert!(!token.is_empty());

    // 5) Token authenticates /me.
    let me = reqwest::blocking::Client::new()
        .get(format!("{base}/api/auth/me"))
        .bearer_auth(&token)
        .send()
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    assert!(me.text().unwrap().contains("dev@b.co"));

    // 6) The RFC 8628 endpoints the CLI/TUI actually calls
    //    (`{issuer}/oauth2/device/code`, `{issuer}/oauth2/token`) answer the
    //    same grants, with OAuth2-shaped bodies.
    let cli = reqwest::blocking::Client::new();
    let created = cli
        .post(format!("{base}/oauth2/device/code"))
        .form(&[("client_id", "astra-cli"), ("scope", "openid")])
        .send()
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let grant: serde_json::Value = created.json().unwrap();
    let cli_device_code = grant["device_code"].as_str().unwrap().to_string();
    assert!(
        grant["verification_uri_complete"]
            .as_str()
            .unwrap()
            .contains(grant["user_code"].as_str().unwrap())
    );

    let pending = cli
        .post(format!("{base}/oauth2/token"))
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", cli_device_code.as_str()),
        ])
        .send()
        .unwrap();
    assert_eq!(pending.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        pending.json::<serde_json::Value>().unwrap()["error"],
        "authorization_pending"
    );

    let approved = client
        .post(format!("{base}/api/auth/device/approve"))
        .header("Origin", base.clone())
        .json(&serde_json::json!({
            "user_code": grant["user_code"].as_str().unwrap()
        }))
        .send()
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);

    let exchanged = cli
        .post(format!("{base}/oauth2/token"))
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", cli_device_code.as_str()),
        ])
        .send()
        .unwrap();
    assert_eq!(exchanged.status(), StatusCode::OK);
    let body: serde_json::Value = exchanged.json().unwrap();
    assert_eq!(body["token_type"], "Bearer");
    let cli_token = body["access_token"].as_str().unwrap().to_string();

    let me = cli
        .get(format!("{base}/api/auth/me"))
        .bearer_auth(&cli_token)
        .send()
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);
}

#[test]
fn cross_site_origin_rejected() {
    let (base, store) = test_server();
    seed_user(&store, "u1", "a@b.co", "password123");

    let resp = reqwest::blocking::Client::new()
        .post(format!("{base}/api/auth/login"))
        .header("Origin", "https://evil.example")
        .json(&serde_json::json!({"email": "a@b.co", "password": "password123"}))
        .send()
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[test]
fn site_pages_serve() {
    let (base, _store) = test_server();
    for p in [
        "/",
        "/login",
        "/authorize",
        "/account",
        "/admin",
        "/assets/fonts.css",
        "/assets/site.css",
        "/assets/marketing.css",
        "/assets/account.css",
        "/assets/auth.js",
        "/assets/admin.js",
        "/assets/site.js",
        "/favicon.svg",
        "/media/fonts/geist-sans-variable.woff2",
        "/media/fonts/geist-mono-variable.woff2",
    ] {
        let resp = reqwest::blocking::get(format!("{base}{p}")).unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{p}");
    }
    // Legacy .html paths 301 to the extensionless route.
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for p in [
        "/login.html",
        "/authorize.html",
        "/account.html",
        "/admin.html",
        "/index.html",
    ] {
        let resp = client.get(format!("{base}{p}")).send().unwrap();
        assert_eq!(resp.status(), StatusCode::MOVED_PERMANENTLY, "{p}");
    }
}

#[test]
fn me_unauthenticated_returns_null() {
    let (base, _store) = test_server();
    let me = reqwest::blocking::get(format!("{base}/api/auth/me")).unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    assert!(me.text().unwrap().contains("\"user\":null"));
}

#[test]
fn account_update_display_name() {
    let (base, store) = test_server();
    seed_user(&store, "u1", "u@b.co", "password123");

    let client = reqwest::blocking::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    client
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({"email": "u@b.co", "password": "password123"}))
        .send()
        .unwrap();

    // Unauthenticated update → 401.
    let unauth = reqwest::blocking::Client::new()
        .post(format!("{base}/api/auth/account"))
        .json(&serde_json::json!({"display_name": "x"}))
        .send()
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    // Authenticated update (with Origin) trims the name.
    let update = client
        .post(format!("{base}/api/auth/account"))
        .header("Origin", base.clone())
        .json(&serde_json::json!({"display_name": "  New Name  "}))
        .send()
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);

    // /me reflects the trimmed name.
    let me = client.get(format!("{base}/api/auth/me")).send().unwrap();
    assert!(me.text().unwrap().contains("\"display_name\":\"New Name\""));

    // Over-long input truncated to 60 chars.
    let long = "a".repeat(100);
    let _ = client
        .post(format!("{base}/api/auth/account"))
        .header("Origin", base.clone())
        .json(&serde_json::json!({"display_name": long}))
        .send()
        .unwrap();
    let u = store.find_user_by_id("u1").unwrap();
    assert_eq!(u.display_name.as_deref().unwrap().chars().count(), 60);
}

#[test]
fn reverse_device_flow_end_to_end() {
    // Authenticated browser user generates a code; the anonymous CLI
    // consumes it and receives a bearer bound to that user — no manual
    // approve step in between.
    let (base, store) = test_server();
    seed_user(&store, "u1", "rev@b.co", "password123");

    let browser = reqwest::blocking::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let login = browser
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({"email": "rev@b.co", "password": "password123"}))
        .send()
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);

    // Browser asks for a one-time code (the reverse endpoint).
    let gen_resp = browser
        .post(format!("{base}/api/auth/device/generate"))
        .header("Origin", base.clone())
        .send()
        .unwrap();
    assert_eq!(gen_resp.status(), StatusCode::OK);
    let gen_body: serde_json::Value = gen_resp.json().unwrap();
    let user_code = gen_body["user_code"].as_str().unwrap().to_string();
    let device_code = gen_body["device_code"].as_str().unwrap().to_string();
    assert_eq!(user_code.len(), 9, "K7Q2-XM9D shape");
    assert!(user_code.contains('-'));

    // The grant is pre-bound to the user (no manual approve).
    let grant = store
        .find_device_by_code(&device_code)
        .expect("grant stored");
    assert_eq!(grant.user_id.as_deref(), Some("u1"));
    assert_eq!(grant.status, "pending");

    // CLI consumes the code (no cookie, no auth header). Mixed-case input
    // is normalised server-side, so a lowercase paste still works.
    let cli = reqwest::blocking::Client::new();
    let consume = cli
        .post(format!("{base}/api/auth/device/consume"))
        .json(&serde_json::json!({"user_code": user_code.to_lowercase()}))
        .send()
        .unwrap();
    assert_eq!(consume.status(), StatusCode::OK);
    let body: serde_json::Value = consume.json().unwrap();
    assert_eq!(body["status"], "approved");
    let token = body["access_token"].as_str().unwrap().to_string();
    assert_eq!(body["user"]["id"], "u1");

    // The minted token authenticates /me.
    let me = cli
        .get(format!("{base}/api/auth/me"))
        .bearer_auth(&token)
        .send()
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    assert!(me.text().unwrap().contains("rev@b.co"));

    // The same code cannot be redeemed twice.
    let second = cli
        .post(format!("{base}/api/auth/device/consume"))
        .json(&serde_json::json!({"user_code": user_code}))
        .send()
        .unwrap();
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
}

#[test]
fn reverse_device_consume_rejects_unbound_grant() {
    // A grant created via the forward `/api/auth/device` endpoint has no
    // bound user; the reverse consume endpoint must refuse it (otherwise an
    // attacker could race the forward flow and hijack the approval).
    let (base, _store) = test_server();

    // Forward flow: anonymous grant, never approved.
    let (_, out) = post_json(&format!("{base}/api/auth/device"), serde_json::json!({}));
    let user_code = out["user_code"].as_str().unwrap().to_string();

    let cli = reqwest::blocking::Client::new();
    let r = cli
        .post(format!("{base}/api/auth/device/consume"))
        .json(&serde_json::json!({"user_code": user_code}))
        .send()
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
}

#[test]
fn device_generate_requires_login() {
    let (base, _store) = test_server();

    // Anonymous: must be rejected by the auth gate before any code is minted.
    let r = reqwest::blocking::Client::new()
        .post(format!("{base}/api/auth/device/generate"))
        .send()
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn tokens_crud() {
    let (base, store) = test_server();
    seed_user(&store, "u1", "t@b.co", "password123");

    let client = reqwest::blocking::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    client
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({"email": "t@b.co", "password": "password123"}))
        .send()
        .unwrap();

    // Create a manual token.
    let create = client
        .post(format!("{base}/api/auth/tokens"))
        .send()
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let token: serde_json::Value = create.json().unwrap();
    let tok = token["token"].as_str().unwrap().to_string();

    // List tokens.
    let list = client
        .get(format!("{base}/api/auth/tokens"))
        .send()
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let body: serde_json::Value = list.json().unwrap();
    assert!(body["tokens"].as_array().unwrap().iter().any(|t| {
        t["token"].as_str() == Some(tok.as_str()) && t["label"].as_str() == Some("manual")
    }));

    // Revoke.
    let del = client
        .delete(format!("{base}/api/auth/tokens"))
        .json(&serde_json::json!({"token": tok}))
        .send()
        .unwrap();
    assert_eq!(del.status(), StatusCode::OK);
    let list2 = client
        .get(format!("{base}/api/auth/tokens"))
        .send()
        .unwrap();
    let body2: serde_json::Value = list2.json().unwrap();
    assert!(
        !body2["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["token"].as_str() == Some(tok.as_str()))
    );
}

#[test]
fn account_and_cli_model_lists_reflect_enabled_routes_and_prices() {
    let (base, store) = test_server();
    seed_user(&store, "u1", "models@b.co", "password123");
    let now = chrono::Utc::now();
    let token = ApiToken {
        token: "cli-model-token".into(),
        user_id: "u1".into(),
        label: Some("test".into()),
        created_at: now,
        expires_at: now + chrono::Duration::days(1),
    };
    store.create_token(&token).unwrap();

    let enabled_route = ModelRoute {
        id: "fast-model".into(),
        display_name: "Fast Model".into(),
        protocol: "openai-compatible".into(),
        base_url: "https://private-upstream.example/v1".into(),
        upstream_model: "private-provider-model".into(),
        context_window: 128_000,
        api_key: Some("private-upstream-key".into()),
        input_price_microusd_per_million: 125_000,
        output_price_microusd_per_million: 750_000,
        enabled: true,
        is_default: true,
    };
    store.upsert_model_route(enabled_route.clone()).unwrap();
    store
        .upsert_model_route(ModelRoute {
            id: "disabled-model".into(),
            display_name: "Disabled Model".into(),
            enabled: false,
            is_default: false,
            ..enabled_route.clone()
        })
        .unwrap();

    let client = reqwest::blocking::Client::new();
    let unauthenticated = client
        .get(format!("{base}/api/auth/models"))
        .send()
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let account_response = client
        .get(format!("{base}/api/auth/models"))
        .bearer_auth(&token.token)
        .send()
        .unwrap();
    assert_eq!(account_response.status(), StatusCode::OK);
    let account_models: serde_json::Value = account_response.json().unwrap();
    let models = account_models["models"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], "fast-model");
    assert_eq!(models[0]["display_name"], "Fast Model");
    assert_eq!(models[0]["input_price_usd_per_million"], 0.125);
    assert_eq!(models[0]["output_price_usd_per_million"], 0.75);
    assert_eq!(models[0]["is_default"], true);
    assert!(models[0].get("base_url").is_none());
    assert!(models[0].get("upstream_model").is_none());
    assert!(models[0].get("api_key").is_none());

    let cli_response = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(&token.token)
        .send()
        .unwrap();
    assert_eq!(cli_response.status(), StatusCode::OK);
    let cli_models: serde_json::Value = cli_response.json().unwrap();
    let cli_models = cli_models["data"].as_array().unwrap();
    assert_eq!(cli_models.len(), 1);
    assert_eq!(cli_models[0]["id"], "fast-model");
    assert_eq!(cli_models[0]["input_price_usd_per_million"], 0.125);
    assert_eq!(cli_models[0]["output_price_usd_per_million"], 0.75);
    assert!(cli_models[0].get("base_url").is_none());

    let updated_route = ModelRoute {
        input_price_microusd_per_million: 200_000,
        output_price_microusd_per_million: 900_000,
        ..enabled_route
    };
    store.upsert_model_route(updated_route).unwrap();
    let refreshed: serde_json::Value = client
        .get(format!("{base}/api/auth/models"))
        .bearer_auth(&token.token)
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(refreshed["models"][0]["input_price_usd_per_million"], 0.2);
    assert_eq!(refreshed["models"][0]["output_price_usd_per_million"], 0.9);
}
