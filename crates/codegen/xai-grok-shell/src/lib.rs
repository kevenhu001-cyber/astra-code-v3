#![allow(
    unused_imports,
    unused_variables,
    unused_mut,
    unreachable_code,
    dead_code
)]
#![warn(unreachable_pub)]
#[cfg(all(test, feature = "dhat-heap"))]
#[global_allocator]
static DHAT_ALLOC: dhat::Alloc = dhat::Alloc;
pub(crate) use xai_grok_telemetry::unified_log;
pub use xai_tracing_macros::{teprintln, timed, tprintln};
pub mod agent;
pub mod auth;
pub mod builtin;
pub use xai_grok_bundle as bundle;
pub mod claude_import;
pub mod claude_import_state;
pub mod cli_models;
pub mod config;
pub use xai_grok_shell_base::cpu_profile;
pub use xai_grok_shell_base::env;
pub mod extensions;
pub use xai_grok_foreign_sessions as foreign_sessions;
pub mod heap_profile;
pub use xai_grok_http as http;
pub mod inspect;
pub mod instrumentation;
pub mod leader;
pub mod managed_config;
pub mod mcp_doctor;
pub use xai_grok_models as models;
pub mod plugin;
pub mod relay;
pub mod remote;
pub mod sampling;
pub mod session;
pub mod terminal;
#[cfg(test)]
pub(crate) mod test_support;

/// Install the JWT crypto provider before any test runs.
///
/// The workspace unifies `jsonwebtoken`'s features across dependents: this
/// crate enables `rust_crypto` while `gcloud-auth` pulls in `aws_lc_rs`, so
/// the crate's automatic provider detection sees BOTH enabled and refuses to
/// pick. Without a pre-main install, whichever test touches JWT first panics
/// and takes the rest of the batch down with it.
#[cfg(test)]
#[ctor::ctor]
fn install_jwt_crypto_provider_for_tests() {
    let _ = jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER.install_default();
}
pub mod tier;
pub mod tools;
pub mod upload;
pub mod util;
