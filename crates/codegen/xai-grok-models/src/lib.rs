//! Default model IDs loaded from `default_models.json` at runtime.
//! Edit that JSON file to change them.
//!
//! At runtime each model is resolved from the first of these that is set: CLI flag, ENV var, config.toml, remote settings, these defaults.
//!
//! The bundled catalog is intentionally empty: the Astra deployment advertises
//! its routing catalog from `/v1/models`, so baking provider slugs into the
//! binary only produces picks that the deployment cannot serve. An empty
//! catalog is a supported state — selection waits for the live catalog
//! (`ModelEntry::pending`) instead of falling back to a stale slug.

use std::sync::LazyLock;

/// The raw JSON, embedded at compile time.
/// It is `pub` because `xai_grok_shell::models` re-exports it and `agent::config` reads it.
pub const DEFAULT_MODELS_JSON: &str = include_str!("../default_models.json");

/// Returned by the accessors when the bundle carries no default.
pub const NO_DEFAULT_MODEL: &str = "";

#[derive(serde::Deserialize)]
struct DefaultModels {
    /// Absent or empty when the bundle ships no default model.
    #[serde(default)]
    default: Option<String>,
    /// Falls back to `default` if not specified in JSON.
    #[serde(default)]
    web_search: Option<String>,
    /// Falls back to `default` if not specified in JSON.
    #[serde(default)]
    image_description: Option<String>,
    /// Falls back to `default` if not specified in JSON.
    #[serde(default)]
    session_summary: Option<String>,
    #[serde(default)]
    models: Vec<DefaultModelEntry>,
}

#[derive(serde::Deserialize)]
struct DefaultModelEntry {
    model: String,
}

static DEFAULTS: LazyLock<DefaultModels> = LazyLock::new(|| {
    let defaults: DefaultModels =
        serde_json::from_str(DEFAULT_MODELS_JSON).expect("default_models.json: invalid JSON");

    // Baked-in JSON: a mismatch here is a developer error. An empty catalog (or
    // an empty `default`) is valid and simply means "no bundled default".
    let model_ids: Vec<&str> = defaults.models.iter().map(|m| m.model.as_str()).collect();
    if let Some(named) = defaults.default.as_deref().filter(|d| !d.is_empty()) {
        assert!(
            model_ids.contains(&named),
            "default_models.json: 'default' is '{named}' but 'models' array only has {model_ids:?}",
        );
    }

    defaults
});

/// Primary model for coding tasks and general fallback.
///
/// Empty when the bundle ships no built-in catalog; callers treat an empty
/// slug as "unset" and defer to the live catalog.
pub fn default_model() -> &'static str {
    DEFAULTS.default.as_deref().unwrap_or(NO_DEFAULT_MODEL)
}

fn or_default(override_slug: Option<&'static str>) -> &'static str {
    override_slug
        .filter(|slug| !slug.is_empty())
        .unwrap_or_else(default_model)
}

/// Model for web search tool synthesis. Falls back to default model.
pub fn default_web_search_model() -> &'static str {
    or_default(DEFAULTS.web_search.as_deref())
}

/// Model for image describe. Falls back to default model.
pub fn default_image_description_model() -> &'static str {
    or_default(DEFAULTS.image_description.as_deref())
}

/// Model for session title generation. Falls back to default model.
pub fn default_session_summary_model() -> &'static str {
    or_default(DEFAULTS.session_summary.as_deref())
}
