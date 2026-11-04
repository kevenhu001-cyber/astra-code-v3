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
fn cloud_gateway_routes_origin_base_and_full_endpoint_without_404() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(|headers: axum::http::HeaderMap, axum::Json(body): axum::Json<serde_json::Value>| async move {
            assert_eq!(headers["authorization"], "Bearer upstream-test-key");
            assert_eq!(body["model"], "provider-model");
            axum::Json(serde_json::json!({
                "id": "test-completion",
                "object": "chat.completion",
                "model": "provider-model",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hello"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1}
            }))
        }),
    );
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = std::thread::spawn(move || {
        rt.block_on(async {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
    });
    let (base, store) = test_server();
    seed_user(&store, "cloud-user", "cloud@example.com", "password123");
    let now = chrono::Utc::now();
    store
        .create_token(&ApiToken {
            token: "cloud-test-token".into(),
            user_id: "cloud-user".into(),
            label: None,
            created_at: now,
            expires_at: now + chrono::Duration::days(1),
        })
        .unwrap();
    let client = reqwest::blocking::Client::new();
    for api_base in [
        upstream.clone(),
        format!("{upstream}/v1"),
        format!("{upstream}/v1/chat/completions/"),
    ] {
        store
            .upsert_model_route(ModelRoute {
                id: "cloud-model".into(),
                display_name: "Cloud Model".into(),
                protocol: "openai-compatible".into(),
                base_url: api_base.clone(),
                upstream_model: "provider-model".into(),
                context_window: 128_000,
                api_key: Some("upstream-test-key".into()),
                input_price_microusd_per_million: 0,
                output_price_microusd_per_million: 0,
                enabled: true,
                is_default: true,
            })
            .unwrap();
        let response = client
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth("cloud-test-token")
            .json(&serde_json::json!({
                "model": "cloud-model", "messages": [{"role": "user", "content": "Hi"}]
            }))
            .send()
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "configured base: {api_base}"
        );
        let body: serde_json::Value = response.json().unwrap();
        assert_eq!(body["model"], "provider-model");
        assert_eq!(body["choices"][0]["message"]["content"], "Hello");
    }
    let _ = stop.send(());
    server.join().unwrap();
}

#[test]
fn gateway_preserves_three_native_protocols_json_streams_and_billing() {
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use serde_json::{Value, json};

    const NATIVE_ERROR: &str =
        " {\"error\":{\"type\":\"native_error\",\"message\":\"upstream model missing\"}} ";
    let cases = [
        (
            "openai-compatible",
            "/chat/completions",
            "chat_completions",
            json!({"model": "public", "messages": [{"role": "user", "content": "Hello"}], "stream_options": {"include_usage": true}, "native_extension": {"keep": true}}),
            " {\"id\":\"chat_1\",\"model\":\"provider-model\",\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"Hello\"}}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2},\"native_extension\":true} ",
            ": keep-alive\r\n\r\ndata: {\"model\":\"provider-model\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"echo\",\"arguments\":\"{}\"}}]}}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n",
        ),
        (
            "openai-responses",
            "/responses",
            "responses",
            json!({"model": "public", "instructions": "Stay native", "input": [{"role": "user", "content": [{"type": "input_text", "text": "Hello"}]}], "tools": [{"type": "function", "name": "echo", "parameters": {"type": "object"}}], "native_extension": {"keep": true}}),
            " {\"id\":\"resp_1\",\"object\":\"response\",\"model\":\"provider-model\",\"status\":\"completed\",\"output\":[{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"echo\",\"arguments\":\"{}\"}],\"usage\":{\"input_tokens\":3,\"output_tokens\":2}} ",
            "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"echo\",\"arguments\":\"{}\"}}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"model\":\"provider-model\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\n",
        ),
        (
            "anthropic-messages",
            "/messages",
            "messages",
            json!({"model": "public", "max_tokens": 100, "system": [{"type": "text", "text": "Stay native"}], "messages": [{"role": "user", "content": [{"type": "text", "text": "Hello"}]}], "tools": [{"name": "echo", "input_schema": {"type": "object"}}], "native_extension": {"keep": true}}),
            " {\"id\":\"msg_1\",\"type\":\"message\",\"model\":\"provider-model\",\"content\":[{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"echo\",\"input\":{}}],\"stop_reason\":\"tool_use\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2}} ",
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"provider-model\",\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":2}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ),
    ];
    let mut app = Router::new();
    for (protocol, suffix, _, payload, native_json, native_sse) in &cases {
        let protocol = *protocol;
        let payload = payload.clone();
        let native_json = *native_json;
        let native_sse = *native_sse;
        app = app.route(
            &format!("/v1{suffix}"),
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let mut expected = payload.clone();
                async move {
                    if protocol == "anthropic-messages" {
                        assert_eq!(headers["x-api-key"], "provider-test-key");
                        assert_eq!(headers["anthropic-version"], "2023-06-01");
                        assert_eq!(headers["anthropic-beta"], "test-beta");
                        assert!(headers.get("authorization").is_none());
                    } else {
                        assert_eq!(headers["authorization"], "Bearer provider-test-key");
                        assert_eq!(headers["openai-beta"], "test-beta");
                    }
                    let stream = body["stream"] == true;
                    let error = body["native_error"] == true;
                    expected["model"] = json!("provider-model");
                    expected["stream"] = json!(stream);
                    if error {
                        expected["native_error"] = json!(true);
                    }
                    assert_eq!(body, expected, "native request was changed");
                    (
                        if error {
                            StatusCode::NOT_FOUND
                        } else {
                            StatusCode::OK
                        },
                        [(
                            axum::http::header::CONTENT_TYPE,
                            if stream {
                                "text/event-stream"
                            } else {
                                "application/json"
                            },
                        )],
                        if error {
                            NATIVE_ERROR
                        } else if stream {
                            native_sse
                        } else {
                            native_json
                        },
                    )
                }
            }),
        );
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = std::thread::spawn(move || {
        rt.block_on(async {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
    });
    let (base, store) = test_server();
    seed_user(&store, "native-user", "native@example.com", "password123");
    store
        .adjust_balance("native-user", 1_000, "test credit".into(), None, 0, 0)
        .unwrap();
    let now = chrono::Utc::now();
    store
        .create_token(&ApiToken {
            token: "native-test-token".into(),
            user_id: "native-user".into(),
            label: None,
            created_at: now,
            expires_at: now + chrono::Duration::days(1),
        })
        .unwrap();
    let client = reqwest::blocking::Client::new();
    for (index, (protocol, suffix, backend, payload, native_json, native_sse)) in
        cases.iter().enumerate()
    {
        let id = format!("native-{index}");
        store
            .upsert_model_route(ModelRoute {
                id: id.clone(),
                display_name: id.clone(),
                protocol: (*protocol).into(),
                base_url: upstream.clone(),
                upstream_model: "provider-model".into(),
                context_window: 128_000,
                api_key: Some("provider-test-key".into()),
                input_price_microusd_per_million: 1_000_000,
                output_price_microusd_per_million: 1_000_000,
                enabled: true,
                is_default: index == 0,
            })
            .unwrap();
        for stream in [false, true] {
            let mut request = payload.clone();
            request["model"] = json!(id);
            request["stream"] = json!(stream);
            let response = client
                .post(format!("{base}/v1{suffix}"))
                .bearer_auth("native-test-token")
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", "test-beta")
                .header("openai-beta", "test-beta")
                .json(&request)
                .send()
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{protocol}");
            assert_eq!(
                response.bytes().unwrap().as_ref(),
                if stream {
                    native_sse.as_bytes()
                } else {
                    native_json.as_bytes()
                },
                "{protocol} response was converted"
            );
        }
        let mut request = payload.clone();
        request["model"] = json!(id);
        request["stream"] = json!(false);
        request["native_error"] = json!(true);
        let response = client
            .post(format!("{base}/v1{suffix}"))
            .bearer_auth("native-test-token")
            .header("anthropic-beta", "test-beta")
            .header("openai-beta", "test-beta")
            .json(&request)
            .send()
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.text().unwrap(), NATIVE_ERROR);
        let wrong_suffix = cases[(index + 1) % cases.len()].1;
        let mismatch = client
            .post(format!("{base}/v1{wrong_suffix}"))
            .bearer_auth("native-test-token")
            .json(&request)
            .send()
            .unwrap();
        assert_eq!(mismatch.status(), StatusCode::BAD_REQUEST);
        let catalog: Value = client
            .get(format!("{base}/v1/models"))
            .bearer_auth("native-test-token")
            .send()
            .unwrap()
            .json()
            .unwrap();
        let entry = catalog["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == id)
            .unwrap();
        assert_eq!(entry["apiBackend"], *backend);
        assert_eq!(entry["protocol"], *protocol);
        let account: Value = client
            .get(format!("{base}/api/auth/models"))
            .bearer_auth("native-test-token")
            .send()
            .unwrap()
            .json()
            .unwrap();
        let entry = account["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == id)
            .unwrap();
        assert_eq!(entry["api_backend"], *backend);
    }
    assert_eq!(
        store
            .find_user_by_id("native-user")
            .unwrap()
            .balance_microusd,
        970
    );
    let _ = stop.send(());
    server.join().unwrap();
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
