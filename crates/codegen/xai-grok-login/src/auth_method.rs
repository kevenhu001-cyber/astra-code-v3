//! First-party API-key environment primitives.
//!
//! Only the env-key checks the auth subsystem itself needs live here. The ACP
//! `auth_methods` list-building surface (`build_auth_methods`,
//! `AuthMethodsBuildInputs`, `should_advertise_xai_api_key`, ...) stays in
//! `xai_grok_shell::agent::auth_method`, which depends on shell's `ModelEntry`.
//!
//! These keys authenticate **custom model providers**, not the Astra account:
//! signing in has exactly one method (browser login), so no env var can stand
//! in for it.

/// Primary env var for a model-provider API key.
pub const XAI_API_KEY_ENV_VAR: &str = "ASTRA_API_KEY";

/// Pre-rebrand env var names, still read as fallbacks so existing deployments
/// keep working. Never advertised in docs or error messages anymore.
pub const LEGACY_XAI_API_KEY_ENV_VAR: &str = "XAI_API_KEY";
/// Oldest env var name, still read as a last-resort fallback.
pub const LEGACY_GROK_CODE_API_KEY_ENV_VAR: &str = "GROK_CODE_XAI_API_KEY";

/// Read the API key from the environment.
///
/// Checks `ASTRA_API_KEY` first, then the legacy `XAI_API_KEY` and
/// `GROK_CODE_XAI_API_KEY` names for backward compatibility.
pub fn read_xai_api_key_env() -> Result<String, std::env::VarError> {
    std::env::var(XAI_API_KEY_ENV_VAR)
        .or_else(|_| std::env::var(LEGACY_XAI_API_KEY_ENV_VAR))
        .or_else(|_| std::env::var(LEGACY_GROK_CODE_API_KEY_ENV_VAR))
}

/// Returns `true` if any of the accepted API-key env vars is set.
pub fn has_xai_api_key_env() -> bool {
    read_xai_api_key_env().is_ok()
}
