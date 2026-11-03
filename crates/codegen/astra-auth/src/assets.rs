//! Embedded official-site assets (migrated from the Go astra-harness repo,
//! `internal/authsrv/site/`). Kept byte-identical so the front-end and the
//! one-line installer copy buttons keep working unchanged.
//!
//! The site was rebuilt on the Socrates marketing system's design language
//! (see `site/assets/site.css`), which split the front end into a shared
//! foundation, a landing sheet, an account sheet and a type sheet. The font
//! binaries are vendored too, so the pages make no third-party request.

pub const INDEX_HTML: &str = include_str!("../assets/site/index.html");
pub const LOGIN_HTML: &str = include_str!("../assets/site/login.html");
pub const AUTHORIZE_HTML: &str = include_str!("../assets/site/authorize.html");
pub const ACCOUNT_HTML: &str = include_str!("../assets/site/account.html");
pub const ADMIN_HTML: &str = include_str!("../assets/site/admin.html");
pub const FAVICON_SVG: &str = include_str!("../assets/site/favicon.svg");

pub const FONTS_CSS: &str = include_str!("../assets/site/assets/fonts.css");
pub const SITE_CSS: &str = include_str!("../assets/site/assets/site.css");
pub const MARKETING_CSS: &str = include_str!("../assets/site/assets/marketing.css");
pub const ACCOUNT_CSS: &str = include_str!("../assets/site/assets/account.css");

pub const AUTH_JS: &str = include_str!("../assets/site/assets/auth.js");
pub const ADMIN_JS: &str = include_str!("../assets/site/assets/admin.js");
pub const SITE_JS: &str = include_str!("../assets/site/assets/site.js");

/// Geist Sans, variable weight axis 100-900. SIL OFL 1.1; the license text
/// sits next to the binaries in `site/media/fonts/LICENSE.txt`.
pub const GEIST_SANS_WOFF2: &[u8] =
    include_bytes!("../assets/site/media/fonts/geist-sans-variable.woff2");
/// Geist Mono, variable weight axis 100-900. SIL OFL 1.1.
pub const GEIST_MONO_WOFF2: &[u8] =
    include_bytes!("../assets/site/media/fonts/geist-mono-variable.woff2");
