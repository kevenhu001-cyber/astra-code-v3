//! Embedded official-site assets (migrated from the Go astra-harness repo,
//! `internal/authsrv/site/`). Kept byte-identical so the front-end and the
//! one-line installer copy buttons keep working unchanged.

pub const INDEX_HTML: &str = include_str!("../assets/site/index.html");
pub const LOGIN_HTML: &str = include_str!("../assets/site/login.html");
pub const AUTHORIZE_HTML: &str = include_str!("../assets/site/authorize.html");
pub const ACCOUNT_HTML: &str = include_str!("../assets/site/account.html");
pub const FAVICON_SVG: &str = include_str!("../assets/site/favicon.svg");
pub const SITE_CSS: &str = include_str!("../assets/site/assets/site.css");
pub const AUTH_JS: &str = include_str!("../assets/site/assets/auth.js");
