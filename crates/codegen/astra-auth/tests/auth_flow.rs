//! End-to-end HTTP tests mirroring the Go `authsrv_test.go` flows: register →
//! verify → session login, device flow, tokens, account update, CSRF.
//!
//! The server runs on a background thread with its own tokio runtime; the
//! tests drive it with `reqwest::blocking` (a blocking client cannot live
//! inside a tokio runtime, so the tests themselves are plain `#[test]`).

use std::sync::Arc;

use astra_auth::mailer::ConsoleMailer;
use astra_auth::server::{Options, Server};
use astra_auth::store::Store;
use axum::http::StatusCode;

/// Start a test server on an ephemeral port. Returns (base_url, store).
fn test_server() -> (String, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("auth.json")).unwrap();
    let srv = Server::new(store.clone(), Arc::new(ConsoleMailer), Options {
        base_url: "http://test.local".to_string(),
        cookie_secure: false,
        cookie_path: "/".to_string(),
    });
    let app = srv.router();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
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
    assert!(out["verification_uri"]
        .as_str()
        .unwrap()
        .contains(&user_code));

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
        "/", "/login", "/authorize", "/account", "/assets/site.css", "/assets/auth.js",
        "/favicon.svg",
    ] {
        let resp = reqwest::blocking::get(format!("{base}{p}")).unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{p}");
    }
    // Legacy .html paths 301 to the extensionless route.
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for p in ["/login.html", "/authorize.html", "/account.html", "/index.html"] {
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
    assert!(me
        .text()
        .unwrap()
        .contains("\"display_name\":\"New Name\""));

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
    let list = client.get(format!("{base}/api/auth/tokens")).send().unwrap();
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
    let list2 = client.get(format!("{base}/api/auth/tokens")).send().unwrap();
    let body2: serde_json::Value = list2.json().unwrap();
    assert!(!body2["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["token"].as_str() == Some(tok.as_str())));
}
