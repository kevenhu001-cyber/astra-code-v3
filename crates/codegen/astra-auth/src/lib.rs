//! Astra official site + account/auth API server.
//!
//! Rust port of the Go `astra-harness` `internal/authsrv` package. Serves the
//! embedded official site (index/login/authorize/account) and the `/api/auth/*`
//! JSON API — registration with email verification, session-cookie login,
//! RFC 8628-style device flow for `astra login`, and API-token management.
//!
//! The on-disk `auth.json` layout matches the Go server, so an existing
//! `/var/lib/astra-auth/auth.json` can be loaded as-is.

pub mod assets;
pub mod mailer;
pub mod password;
pub mod server;
pub mod store;
