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

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("astra-auth");
        }
    }
    if let Ok(dir) = std::env::var("HOME") {
        return PathBuf::from(dir).join(".config").join("astra-auth");
    }
    PathBuf::from(".astra-auth")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();

    let addr = parse_flag(&args, "addr", env_or("ASTRA_AUTH_ADDR", ":8080"));
    let base_url = parse_flag(&args, "base-url", env_or("ASTRA_AUTH_BASE_URL", "http://localhost:8080"));
    let data_dir = parse_flag(&args, "data-dir", env_or("ASTRA_AUTH_DATA_DIR", default_data_dir().to_string_lossy().to_string()));
    let cookie_path = parse_flag(&args, "cookie-path", env_or("ASTRA_AUTH_COOKIE_PATH", "/"));
    let cookie_secure = parse_flag(&args, "cookie-secure", false);

    let store = Store::open(PathBuf::from(&data_dir).join("auth.json"))?;

    let mailer: Arc<dyn Mailer> = {
        let smtp = SMTPMailer::from_env();
        if smtp.enabled() {
            tracing::info!("mailer: smtp ({})", smtp_host(&smtp));
            Arc::new(smtp)
        } else {
            tracing::info!("mailer: console (verification links printed to this log); set SMTP_HOST/SMTP_USER/SMTP_PASS to send real email");
            Arc::new(ConsoleMailer)
        }
    };

    let opts = Options {
        base_url,
        cookie_secure,
        cookie_path,
    };
    let srv = Server::new(store, mailer, opts);
    let router = srv.router();

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow::anyhow!("bind {addr}: {e}"))?;
    tracing::info!("astra-auth listening on {addr} (site + API at {})", opts.base_url);
    axum::serve(listener, router)
        .await
        .map_err(|e| anyhow::anyhow!("server: {e}"))?;
    Ok(())
}

fn smtp_host(m: &SMTPMailer) -> String {
    // SMTPMailer fields are private; expose host via a helper on the type.
    m.host().to_string()
}
