//! OAuth2/OIDC token primitives shared by the Astra sign-in flow.
//!
//! The customer-IdP (enterprise SSO) login was removed: enterprise IdP is no
//! longer configurable or reachable. What remains is the token machinery the
//! Astra device-code login and the background refresher use — the interaction
//! with a customer identity provider is gone, the token/JWT helpers are not.
pub mod protocol;
pub mod refresh;
#[cfg(test)]
mod test_helpers;
pub use protocol::{
    enforce_login_principal, login_principal_policy, peek_access_token_principal_id,
};
pub use protocol::{peek_access_token_principal, with_alpha_test_key};
pub use refresh::{OidcRefreshResult, oidc_token_exchange};
