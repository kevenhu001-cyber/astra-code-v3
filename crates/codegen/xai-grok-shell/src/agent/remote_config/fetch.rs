//! Model-catalog fetch transport.

use chrono::{DateTime, Utc};
use indexmap::IndexMap;

use super::{ModelFetchAuth, ModelsCacheManager, ModelsCacheScope};
use crate::agent::config::{self, ModelEntry};
use crate::remote::{FetchModelsResult, ModelSource, active_model_source};
use xai_grok_login::GrokAuth;

pub(crate) fn build_prefetched_map(
    models: Vec<config::ModelEntryConfig>,
    api_base_url_override: Option<String>,
) -> IndexMap<String, ModelEntry> {
    let mut map: IndexMap<String, ModelEntry> = IndexMap::with_capacity(models.len());
    for m in models {
        let key = m.id.clone().unwrap_or_else(|| m.model.clone());
        let info = config::ModelInfo::from_config(&m);
        let entry = ModelEntry {
            info,
            mtls_cert_dir: None,
            api_key: None,
            env_key: None,
            auth_provider: None,
            api_base_url: m.api_base_url.clone().or(api_base_url_override.clone()),
        };
        map.insert(key, entry);
    }
    map
}

pub(crate) fn prefetch_models_blocking(
    endpoints: &config::EndpointsConfig,
    auth: Option<&GrokAuth>,
    fetch_auth: ModelFetchAuth,
) -> Option<IndexMap<String, ModelEntry>> {
    prefetch_models_blocking_gated(
        endpoints,
        auth,
        fetch_auth,
        crate::util::config::resolve_remote_fetch_enabled(),
    )
}

fn prefetch_models_blocking_gated(
    endpoints: &config::EndpointsConfig,
    auth: Option<&GrokAuth>,
    fetch_auth: ModelFetchAuth,
    remote_fetch_enabled: bool,
) -> Option<IndexMap<String, ModelEntry>> {
    fetch_models_uncommitted(endpoints, auth, fetch_auth, remote_fetch_enabled).commit()
}

/// A models fetch not yet written to the disk cache; the commit point decides
/// whether any state lands.
pub(in crate::agent::remote_config) enum ModelsPrefetch {
    Cached(IndexMap<String, ModelEntry>),
    Fetched(ModelsCacheWrite),
    Unavailable,
}

impl ModelsPrefetch {
    fn commit(self) -> Option<IndexMap<String, ModelEntry>> {
        match self {
            Self::Cached(models) => Some(models),
            Self::Fetched(write) => Some(write.commit()),
            Self::Unavailable => None,
        }
    }
}

pub(in crate::agent::remote_config) struct ModelsCacheWrite {
    models: IndexMap<String, ModelEntry>,
    etag: Option<String>,
    scope: ModelsCacheScope,
    fetched_at: DateTime<Utc>,
}

impl ModelsCacheWrite {
    pub(in crate::agent::remote_config) fn commit(self) -> IndexMap<String, ModelEntry> {
        ModelsCacheManager::new().persist(
            &self.models,
            self.etag.as_deref(),
            &self.scope,
            self.fetched_at,
        );
        self.models
    }

    /// The fetched catalog without persisting it: serve a live session not yet
    /// on disk without writing into the auth-method/origin-scoped cache.
    pub(in crate::agent::remote_config) fn into_models(self) -> IndexMap<String, ModelEntry> {
        self.models
    }
}

pub(in crate::agent::remote_config) fn fetch_models_uncommitted(
    endpoints: &config::EndpointsConfig,
    auth: Option<&GrokAuth>,
    fetch_auth: ModelFetchAuth,
    remote_fetch_enabled: bool,
) -> ModelsPrefetch {
    fetch_models_uncommitted_inner(
        endpoints,
        auth,
        fetch_auth,
        remote_fetch_enabled,
        true,
        &ModelsCacheManager::new(),
    )
}

/// Explicit refreshes must reach the server even while the startup cache is fresh.
pub(in crate::agent::remote_config) fn refresh_models_blocking(
    endpoints: &config::EndpointsConfig,
    auth: Option<&GrokAuth>,
    fetch_auth: ModelFetchAuth,
) -> Option<IndexMap<String, ModelEntry>> {
    fetch_models_uncommitted_inner(
        endpoints,
        auth,
        fetch_auth,
        crate::util::config::resolve_remote_fetch_enabled(),
        false,
        &ModelsCacheManager::new(),
    )
    .commit()
}

fn fetch_models_uncommitted_inner(
    endpoints: &config::EndpointsConfig,
    auth: Option<&GrokAuth>,
    fetch_auth: ModelFetchAuth,
    remote_fetch_enabled: bool,
    use_cache: bool,
    cache: &ModelsCacheManager,
) -> ModelsPrefetch {
    let source = active_model_source(endpoints, fetch_auth);
    let scope = ModelsCacheScope::resolve(endpoints, fetch_auth, auth);

    if use_cache && let Some(cached) = cache.load_fresh(&scope) {
        return ModelsPrefetch::Cached(cached.models);
    }

    if !remote_fetch_enabled {
        tracing::info!("models fetch skipped: remote_fetch disabled");
        return ModelsPrefetch::Unavailable;
    }

    let _timer = crate::instrumentation_timer!("startup.fetch_models_blocking");
    let fetched_at = Utc::now();
    match source.fetch(auth) {
        Ok(FetchModelsResult { models, etag }) => {
            let api_base_url_override = match fetch_auth {
                ModelFetchAuth::ApiKey => Some(endpoints.xai_api_base_url.clone()),
                _ => None,
            };
            let map = build_prefetched_map(models, api_base_url_override);

            tracing::info!(count = map.len(), etag = ?etag, "Prefetched models");
            ModelsPrefetch::Fetched(ModelsCacheWrite {
                models: map,
                etag,
                scope,
                fetched_at,
            })
        }
        Err(e) => {
            tracing::warn!(error = ?e, "Failed to fetch models");
            ModelsPrefetch::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refresh_bypasses_fresh_cache_and_accepts_empty_catalog() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(|| async { axum::Json(serde_json::json!({"data": []})) }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::task::spawn_blocking(move || {
            let tmp = tempfile::tempdir().unwrap();
            let cache = ModelsCacheManager {
                path: tmp.path().join("models_cache.json"),
                ttl: std::time::Duration::from_secs(300),
            };
            let endpoints = config::EndpointsConfig {
                models_base_url: Some(base),
                ..Default::default()
            };
            let auth = GrokAuth::test_default();
            let scope = ModelsCacheScope::resolve(&endpoints, ModelFetchAuth::ApiKey, Some(&auth));
            let stale = IndexMap::from([(
                "removed-model".into(),
                ModelEntry::fallback("removed-model", &endpoints),
            )]);
            cache.persist(&stale, None, &scope, Utc::now());
            assert!(matches!(
                fetch_models_uncommitted_inner(
                    &endpoints,
                    Some(&auth),
                    ModelFetchAuth::ApiKey,
                    true,
                    true,
                    &cache
                ),
                ModelsPrefetch::Cached(_)
            ));
            let refreshed = fetch_models_uncommitted_inner(
                &endpoints,
                Some(&auth),
                ModelFetchAuth::ApiKey,
                true,
                false,
                &cache,
            );
            let ModelsPrefetch::Fetched(write) = refreshed else {
                panic!("refresh must fetch rather than reuse a fresh cache");
            };
            assert!(
                write.into_models().is_empty(),
                "disabled routes must disappear from the catalog"
            );
        })
        .await
        .unwrap();
        server.abort();
    }
}
