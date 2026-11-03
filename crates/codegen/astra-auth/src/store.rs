//! Persistent auth state, stored as a single JSON document (matching the Go
//! `astra-auth` schema so an existing `/var/lib/astra-auth/auth.json` can be
//! loaded as-is). Written atomically (tmp + rename) on every mutation.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A verified account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub email: String,
    #[serde(rename = "password_hash")]
    pub password_hash: String,
    #[serde(rename = "display_name", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(rename = "created_at")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "verified_at", skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<DateTime<Utc>>,
    /// Accounts can be promoted by configuring `ASTRA_AUTH_ADMIN_EMAIL`.
    #[serde(default, rename = "is_admin")]
    pub is_admin: bool,
    /// Credit balance in micro-USD (1 USD = 1,000,000 units).
    #[serde(default, rename = "balance_microusd")]
    pub balance_microusd: i64,
}

/// A public model name and its private upstream route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoute {
    /// Stable model id exposed to CLI clients.
    pub id: String,
    pub display_name: String,
    /// `openai-compatible`, `openai-responses`, or `anthropic-messages`.
    pub protocol: String,
    pub base_url: String,
    pub upstream_model: String,
    #[serde(default = "default_context_window")]
    pub context_window: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// USD per million input/output tokens, stored as micro-USD to avoid floats.
    #[serde(default)]
    pub input_price_microusd_per_million: u64,
    #[serde(default)]
    pub output_price_microusd_per_million: u64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub is_default: bool,
}

fn default_true() -> bool {
    true
}

fn default_context_window() -> u64 {
    128_000
}

/// Credit or usage entry shown in the account ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: String,
    pub user_id: String,
    /// Positive values add credit; negative values record model usage.
    pub delta_microusd: i64,
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    pub created_at: DateTime<Utc>,
}

/// Pending registration (email + hash stored first; user created on verify).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingRegistration {
    pub email: String,
    #[serde(rename = "password_hash")]
    pub password_hash: String,
    pub token: String,
    #[serde(rename = "expires_at")]
    pub expires_at: DateTime<Utc>,
}

/// Website browser session (sid cookie).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    #[serde(rename = "user_id")]
    pub user_id: String,
    #[serde(rename = "expires_at")]
    pub expires_at: DateTime<Utc>,
}

/// Opaque bearer credential issued to CLI/TUI clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    pub token: String,
    #[serde(rename = "user_id")]
    pub user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(rename = "created_at")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "expires_at")]
    pub expires_at: DateTime<Utc>,
}

/// One device-authorization flow (RFC 8628 style).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceGrant {
    #[serde(rename = "device_code")]
    pub device_code: String,
    #[serde(rename = "user_code")]
    pub user_code: String,
    pub status: String, // pending | approved | expired
    #[serde(rename = "user_id", skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(rename = "created_at")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "expires_at")]
    pub expires_at: DateTime<Utc>,
    #[serde(rename = "approved_at", skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<DateTime<Utc>>,
}

/// Store persists all auth state as a single JSON document.
#[derive(Debug, Default)]
pub struct Store {
    inner: Mutex<StoreInner>,
    path: PathBuf,
}

impl Clone for Store {
    fn clone(&self) -> Self {
        Store {
            // Snapshot the current in-memory state; the clone gets its own
            // mutex (each instance persists to the same `path` on write).
            inner: Mutex::new(self.inner.lock().unwrap().clone()),
            path: self.path.clone(),
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct StoreInner {
    users: Vec<User>,
    pending: Vec<PendingRegistration>,
    sessions: Vec<Session>,
    tokens: Vec<ApiToken>,
    devices: Vec<DeviceGrant>,
    #[serde(default)]
    models: Vec<ModelRoute>,
    #[serde(default)]
    ledger: Vec<LedgerEntry>,
}

/// Normalize an email: lowercase + trim (matches Go's NormalizeEmail).
pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

impl Store {
    /// Open (or create) the store at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let inner = match std::fs::read(&path) {
            Ok(data) => serde_json::from_slice(&data)
                .with_context(|| format!("parse {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let inner = StoreInner::default();
                let json = serde_json::to_vec_pretty(&inner)?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("create {}", parent.display()))?;
                }
                write_atomic(&path, &json)?;
                inner
            }
            Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
        };
        Ok(Store {
            inner: Mutex::new(inner),
            path,
        })
    }

    fn save_locked(&self, inner: &StoreInner) -> Result<()> {
        let json = serde_json::to_vec_pretty(inner)?;
        write_atomic(&self.path, &json)
    }

    // --- users ---

    pub fn find_user_by_email(&self, email: &str) -> Option<User> {
        let email = normalize_email(email);
        let inner = self.inner.lock().unwrap();
        inner.users.iter().find(|u| u.email == email).cloned()
    }

    pub fn find_user_by_id(&self, id: &str) -> Option<User> {
        let inner = self.inner.lock().unwrap();
        inner.users.iter().find(|u| u.id == id).cloned()
    }

    pub fn create_user(&self, user: &User) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.users.push(user.clone());
        self.save_locked(&inner)
    }

    /// Apply a mutator to the user with the given id. Returns false if absent.
    pub fn update_user(&self, id: &str, mutate: impl FnOnce(&mut User)) -> Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        let Some(u) = inner.users.iter_mut().find(|u| u.id == id) else {
            return Ok(false);
        };
        mutate(u);
        self.save_locked(&inner)?;
        Ok(true)
    }

    /// Promote the configured administrator account once it exists.
    pub fn promote_admin_by_email(&self, email: &str) -> Result<bool> {
        let email = normalize_email(email);
        if email.is_empty() {
            return Ok(false);
        }
        let mut inner = self.inner.lock().unwrap();
        let Some(user) = inner.users.iter_mut().find(|u| u.email == email) else {
            return Ok(false);
        };
        if user.is_admin {
            return Ok(false);
        }
        user.is_admin = true;
        self.save_locked(&inner)?;
        Ok(true)
    }

    /// Keep the persisted administrator role aligned with the configured email.
    pub fn configure_admin_email(&self, email: &str) -> Result<bool> {
        let email = normalize_email(email);
        let mut inner = self.inner.lock().unwrap();
        let mut changed = false;
        for user in &mut inner.users {
            let is_admin = !email.is_empty() && user.email == email;
            if user.is_admin != is_admin {
                user.is_admin = is_admin;
                changed = true;
            }
        }
        if changed {
            self.save_locked(&inner)?;
        }
        Ok(changed)
    }

    pub fn users(&self) -> Vec<User> {
        let inner = self.inner.lock().unwrap();
        let mut users = inner.users.clone();
        users.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        users
    }

    // --- pending registrations ---

    pub fn find_pending(&self, email: &str) -> Option<PendingRegistration> {
        let email = normalize_email(email);
        let inner = self.inner.lock().unwrap();
        inner.pending.iter().find(|p| p.email == email).cloned()
    }

    pub fn find_pending_by_token(&self, token: &str) -> Option<PendingRegistration> {
        let inner = self.inner.lock().unwrap();
        inner.pending.iter().find(|p| p.token == token).cloned()
    }

    pub fn upsert_pending(&self, p: &PendingRegistration) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let email = normalize_email(&p.email);
        if let Some(existing) = inner.pending.iter_mut().find(|x| x.email == email) {
            *existing = p.clone();
        } else {
            inner.pending.push(p.clone());
        }
        self.save_locked(&inner)
    }

    pub fn delete_pending(&self, email: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let email = normalize_email(email);
        inner.pending.retain(|p| p.email != email);
        self.save_locked(&inner)
    }

    // --- sessions ---

    pub fn create_session(&self, s: &Session) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.sessions.push(s.clone());
        self.save_locked(&inner)
    }

    /// Return the session if present and unexpired.
    pub fn find_session(&self, token: &str) -> Option<Session> {
        let now = Utc::now();
        let inner = self.inner.lock().unwrap();
        inner
            .sessions
            .iter()
            .find(|s| s.token == token && s.expires_at > now)
            .cloned()
    }

    pub fn delete_session(&self, token: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.sessions.retain(|s| s.token != token);
        self.save_locked(&inner)
    }

    // --- api tokens ---

    pub fn find_token(&self, token: &str) -> Option<ApiToken> {
        let now = Utc::now();
        let inner = self.inner.lock().unwrap();
        inner
            .tokens
            .iter()
            .find(|t| t.token == token && t.expires_at > now)
            .cloned()
    }

    pub fn create_token(&self, t: &ApiToken) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.tokens.push(t.clone());
        self.save_locked(&inner)
    }

    pub fn tokens_for_user(&self, user_id: &str) -> Vec<ApiToken> {
        let now = Utc::now();
        let inner = self.inner.lock().unwrap();
        inner
            .tokens
            .iter()
            .filter(|t| t.user_id == user_id && t.expires_at > now)
            .cloned()
            .collect()
    }

    pub fn revoke_token(&self, token: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.tokens.retain(|t| t.token != token);
        self.save_locked(&inner)
    }

    // --- model routes ---

    pub fn model_routes(&self) -> Vec<ModelRoute> {
        self.inner.lock().unwrap().models.clone()
    }

    pub fn model_route(&self, id: &str) -> Option<ModelRoute> {
        self.inner
            .lock()
            .unwrap()
            .models
            .iter()
            .find(|route| route.id == id)
            .cloned()
    }

    pub fn upsert_model_route(&self, mut route: ModelRoute) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let existing_default = inner
            .models
            .iter()
            .find(|existing| existing.is_default)
            .map(|existing| existing.id.clone());
        let removing_existing_default =
            existing_default.as_deref() == Some(route.id.as_str()) && !route.is_default;
        if removing_existing_default {
            if let Some(replacement) = inner
                .models
                .iter_mut()
                .find(|existing| existing.id != route.id && existing.enabled)
            {
                replacement.is_default = true;
            } else {
                route.is_default = true;
            }
        }
        let should_be_default = route.is_default
            || existing_default.is_none()
            || !inner.models.iter().any(|existing| existing.is_default);
        if should_be_default {
            for existing in &mut inner.models {
                existing.is_default = false;
            }
            route.is_default = true;
        }
        if let Some(existing) = inner.models.iter_mut().find(|r| r.id == route.id) {
            *existing = route;
        } else {
            inner.models.push(route);
        }
        self.save_locked(&inner)
    }

    pub fn delete_model_route(&self, id: &str) -> Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        let was_default = inner
            .models
            .iter()
            .any(|route| route.id == id && route.is_default);
        let before = inner.models.len();
        inner.models.retain(|route| route.id != id);
        if inner.models.len() == before {
            return Ok(false);
        }
        if was_default && let Some(route) = inner.models.iter_mut().find(|route| route.enabled) {
            route.is_default = true;
        }
        self.save_locked(&inner)?;
        Ok(true)
    }

    // --- balance and usage ledger ---

    pub fn ledger_for_user(&self, user_id: &str, limit: usize) -> Vec<LedgerEntry> {
        let inner = self.inner.lock().unwrap();
        let mut entries: Vec<_> = inner
            .ledger
            .iter()
            .filter(|entry| entry.user_id == user_id)
            .cloned()
            .collect();
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        entries.truncate(limit);
        entries
    }

    pub fn adjust_balance(
        &self,
        user_id: &str,
        delta_microusd: i64,
        note: String,
        model: Option<String>,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        let Some(user) = inner.users.iter_mut().find(|user| user.id == user_id) else {
            return Ok(false);
        };
        user.balance_microusd = user.balance_microusd.saturating_add(delta_microusd);
        inner.ledger.push(LedgerEntry {
            id: crate::password::random_hex(12),
            user_id: user_id.to_string(),
            delta_microusd,
            note,
            model,
            input_tokens,
            output_tokens,
            created_at: Utc::now(),
        });
        self.save_locked(&inner)?;
        Ok(true)
    }
    // --- device grants ---

    pub fn create_device(&self, g: &DeviceGrant) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.devices.push(g.clone());
        self.save_locked(&inner)
    }

    pub fn find_device_by_code(&self, device_code: &str) -> Option<DeviceGrant> {
        let inner = self.inner.lock().unwrap();
        inner
            .devices
            .iter()
            .find(|d| d.device_code == device_code)
            .cloned()
    }

    pub fn find_device_by_user_code(&self, user_code: &str) -> Option<DeviceGrant> {
        let user_code = user_code.trim().to_uppercase();
        let inner = self.inner.lock().unwrap();
        inner
            .devices
            .iter()
            .find(|d| d.user_code == user_code)
            .cloned()
    }

    pub fn update_device(
        &self,
        device_code: &str,
        mutate: impl FnOnce(&mut DeviceGrant),
    ) -> Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        let Some(d) = inner
            .devices
            .iter_mut()
            .find(|d| d.device_code == device_code)
        else {
            return Ok(false);
        };
        mutate(d);
        self.save_locked(&inner)?;
        Ok(true)
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("open {}", tmp.display()))?;
        file.write_all(data)
            .with_context(|| format!("write {}", tmp.display()))?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, data).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename onto {}", path.display()))?;
    Ok(())
}
