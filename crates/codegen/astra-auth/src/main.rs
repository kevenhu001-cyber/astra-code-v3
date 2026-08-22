//! Command `astra-auth` serves the Astra official site and the account/auth
//! API used by `astra login`. Drop-in replacement for the Go astra-auth
//! binary: same flags, same data layout, same wire API.

use std::path::PathBuf;
use std::sync::Arc;

use astra_auth::mailer::{ConsoleMailer, Mailer, SMTPMailer};
use astra_auth::server::{Options, Server};
use astra_auth::store::Store;
use tracing_subscriber::EnvFilter;

/// Parse `--flag value` / `--flag=value` style args into a map.
fn parse_flag<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    let mut out = default;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(v) = a.strip_prefix(&format!("--{name}=")) {
            if let Ok(p) = v.parse() {
                out = p;
            }
        } else if a == &format!("--{name}") && i + 1 < args.len() {
            if let Ok(p) = args[i + 1].parse() {
                out = p;
            }
            i += 1;
        }
        i += 1;
    }
    out
}

fn env_or(key: &str, default: impl AsRef<str>) -> String {
    std::env::var(key).unwrap_or_else(|_| default.as_ref().to_string())
}

fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME")
        && !dir.is_empty()
    {
        return PathBuf::from(dir).join("astra-auth");
    }
    if let Ok(dir) = std::env::var("HOME") {
        return PathBuf::from(dir).join(".config").join("astra-auth");
    }
    PathBuf::from(".astra-auth")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Fast path for `--version` / `--help` used by CI smoke tests.
    // Must not attempt to bind or touch the store.
    if args.iter().any(|a| {
        matches!(
            a.as_str(),
            "--version" | "-V" | "--help" | "-h" | "version" | "help"
        )
    }) {
        println!("astra-auth {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mut addr = parse_flag(&args, "addr", env_or("ASTRA_AUTH_ADDR", "0.0.0.0:8080"));
    // Normalize Go-style `:8080` (empty host) which `tokio::net::TcpListener::bind`
    // cannot resolve on some platforms (err: "failed to lookup address information").
    if addr.starts_with(':') {
        addr = format!("0.0.0.0{addr}");
    }
    let base_url = parse_flag(
        &args,
        "base-url",
        env_or("ASTRA_AUTH_BASE_URL", "http://localhost:8080"),
    );
    let data_dir = parse_flag(
        &args,
        "data-dir",
        env_or("ASTRA_AUTH_DATA_DIR", default_data_dir().to_string_lossy()),
    );
    let cookie_path = parse_flag(&args, "cookie-path", env_or("ASTRA_AUTH_COOKIE_PATH", "/"));
    let cookie_secure = parse_flag(&args, "cookie-secure", false);

    let store = Store::open(PathBuf::from(&data_dir).join("auth.json"))?;

    let mailer: Arc<dyn Mailer> = {
        let smtp = SMTPMailer::from_env();
        if smtp.enabled() {
            tracing::info!("mailer: smtp ({})", smtp_host(&smtp));
            Arc::new(smtp)
        } else {
            tracing::info!(
                "mailer: console (verification links printed to this log); set SMTP_HOST/SMTP_USER/SMTP_PASS to send real email"
            );
            Arc::new(ConsoleMailer)
        }
    };

    let opts = Options {
        base_url,
        cookie_secure,
        cookie_path,
    };
    let srv = Server::new(store, mailer, opts.clone());
    let router = srv.router();

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow::anyhow!("bind {addr}: {e}"))?;
    tracing::info!(
        "astra-auth listening on {addr} (site + API at {})",
        opts.base_url
    );
    axum::serve(listener, router)
        .await
        .map_err(|e| anyhow::anyhow!("server: {e}"))?;
    Ok(())
}

fn smtp_host(m: &SMTPMailer) -> String {
    // SMTPMailer fields are private; expose host via a helper on the type.
    m.host().to_string()
}
