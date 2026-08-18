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
#[derive(Debug, Default, Clone)]
pub struct Store {
    inner: Mutex<StoreInner>,
    path: PathBuf,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct StoreInner {
    users: Vec<User>,
    pending: Vec<PendingRegistration>,
    sessions: Vec<Session>,
    tokens: Vec<ApiToken>,
    devices: Vec<DeviceGrant>,
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
            Ok(data) => {
                serde_json::from_slice(&data).with_context(|| format!("parse {}", path.display()))?
            }
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
        inner
            .users
            .iter()
            .find(|u| u.email == email)
            .cloned()
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

    pub fn update_device(&self, device_code: &str, mutate: impl FnOnce(&mut DeviceGrant)) -> Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        let Some(d) = inner.devices.iter_mut().find(|d| d.device_code == device_code) else {
            return Ok(false);
        };
        mutate(d);
        self.save_locked(&inner)?;
        Ok(true)
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename onto {}", path.display()))?;
    Ok(())
}
