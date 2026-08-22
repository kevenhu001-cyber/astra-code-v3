//! Guided interactive TUI modal for `/connect`.
//!
//! Provides a GUI-like, scrollable, clickable interface to configure custom AI providers:
//! - Provider preset / custom ID
//! - Provider Name
//! - Protocol format (OpenAI Chat Compatible, OpenAI Responses, Anthropic Messages)
//! - Base URL
//! - API Key (with secure mask/unmask toggle and paste support)
//! - Model ID with live upstream model auto-fetching (`[Fetch Upstream Models]`)
//! - Model Display Name
//! - Think-tag injection toggle
//! - Mouse hover/clicks and keyboard navigation

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;
use crate::views::modal_window::{
    self as mw, ModalSizing, ModalWindowConfig, ModalWindowState, Shortcut,
};

/// Canonical provider presets with id, default label, protocol, default endpoint, and think-tag flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderPresetDef {
    pub id: &'static str,
    pub label: &'static str,
    pub protocol: ProtocolBackend,
    pub base_url: &'static str,
    pub injects_think_tags: bool,
}

pub const PRESET_DEFS: &[ProviderPresetDef] = &[
    ProviderPresetDef {
        id: "openai",
        label: "OpenAI (Chat Completions)",
        protocol: ProtocolBackend::ChatCompletions,
        base_url: "https://api.openai.com/v1",
        injects_think_tags: false,
    },
    ProviderPresetDef {
        id: "openai_responses",
        label: "OpenAI (Responses API)",
        protocol: ProtocolBackend::Responses,
        base_url: "https://api.openai.com/v1",
        injects_think_tags: false,
    },
    ProviderPresetDef {
        id: "anthropic",
        label: "Anthropic (Messages API)",
        protocol: ProtocolBackend::Messages,
        base_url: "https://api.anthropic.com/v1",
        injects_think_tags: false,
    },
    ProviderPresetDef {
        id: "topodrive",
        label: "topodrive",
        protocol: ProtocolBackend::ChatCompletions,
        base_url: "https://api.topodrive.top/v1",
        injects_think_tags: false,
    },
    ProviderPresetDef {
        id: "deepseek",
        label: "DeepSeek",
        protocol: ProtocolBackend::ChatCompletions,
        base_url: "https://api.deepseek.com/v1",
        injects_think_tags: false,
    },
    ProviderPresetDef {
        id: "zhipu",
        label: "智谱 Zhipu AI",
        protocol: ProtocolBackend::ChatCompletions,
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        injects_think_tags: false,
    },
    ProviderPresetDef {
        id: "xiaomi",
        label: "小米 Xiaomi (MiMo)",
        protocol: ProtocolBackend::ChatCompletions,
        base_url: "https://api.xiaomimimo.com/v1",
        injects_think_tags: false,
    },
    ProviderPresetDef {
        id: "minimax_cn",
        label: "MiniMax CN",
        protocol: ProtocolBackend::ChatCompletions,
        base_url: "https://api.minimaxi.com/v1",
        injects_think_tags: true,
    },
    ProviderPresetDef {
        id: "zai",
        label: "zAI (Zhipu International)",
        protocol: ProtocolBackend::ChatCompletions,
        base_url: "https://api.z.ai/api/paas/v4",
        injects_think_tags: false,
    },
    ProviderPresetDef {
        id: "custom",
        label: "Custom / Other Provider",
        protocol: ProtocolBackend::ChatCompletions,
        base_url: "",
        injects_think_tags: false,
    },
];

/// Presets slice mirrored for legacy compatibility.
pub const PRESETS: &[(&str, &str)] = &[
    ("openai", "https://api.openai.com/v1"),
    ("openai_responses", "https://api.openai.com/v1"),
    ("anthropic", "https://api.anthropic.com/v1"),
    ("xai", "https://api.topodrive.top/v1"),
    ("deepseek", "https://api.deepseek.com/v1"),
    ("zhipu", "https://open.bigmodel.cn/api/paas/v4"),
    ("xiaomi", "https://api.xiaomimimo.com/v1"),
    ("minimax_cn", "https://api.minimaxi.com/v1"),
    ("zai", "https://api.z.ai/api/paas/v4"),
    ("custom", ""),
];

/// Protocol backend format choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolBackend {
    ChatCompletions,
    Responses,
    Messages,
}

impl ProtocolBackend {
    pub const ALL: &'static [ProtocolBackend] = &[
        ProtocolBackend::ChatCompletions,
        ProtocolBackend::Responses,
        ProtocolBackend::Messages,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }

    pub fn display_label(self) -> &'static str {
        match self {
            Self::ChatCompletions => "OpenAI Chat Compatible (/v1/chat/completions)",
            Self::Responses => "OpenAI Responses (/v1/responses)",
            Self::Messages => "Anthropic Messages (/v1/messages)",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::ChatCompletions => "OpenAI Chat",
            Self::Responses => "OpenAI Responses",
            Self::Messages => "Anthropic Messages",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "responses" => Self::Responses,
            "messages" | "anthropic" => Self::Messages,
            _ => Self::ChatCompletions,
        }
    }
}

/// Result of running the wizard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectWizardResult {
    pub provider: String,
    pub model_id: String,
    pub display_name: String,
    pub api_key: String,
    pub base_url: String,
    pub protocol: ProtocolBackend,
    pub injects_think_tags: bool,
}

/// Focused fields in the connect form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Preset,
    ProviderName,
    Protocol,
    Url,
    ApiKey,
    ModelId,
    FetchModels,
    ModelName,
    InjectThinkTags,
    Submit,
}

impl Field {
    pub const ORDER: &'static [Field] = &[
        Field::Preset,
        Field::ProviderName,
        Field::Protocol,
        Field::Url,
        Field::ApiKey,
        Field::ModelId,
        Field::FetchModels,
        Field::ModelName,
        Field::InjectThinkTags,
        Field::Submit,
    ];

    pub fn index(self) -> usize {
        Self::ORDER.iter().position(|f| *f == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> Field {
        Self::ORDER[i.min(Self::ORDER.len() - 1)]
    }

    pub fn next(self) -> Field {
        let i = self.index();
        if i + 1 >= Self::ORDER.len() {
            Self::ORDER[0]
        } else {
            Self::ORDER[i + 1]
        }
    }

    pub fn prev(self) -> Field {
        let i = self.index();
        if i == 0 {
            Self::ORDER[Self::ORDER.len() - 1]
        } else {
            Self::ORDER[i - 1]
        }
    }
}

/// Wizard outcome after input handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardOutcome {
    Unhandled,
    Changed,
    Closed,
    Submitted(ConnectWizardResult),
}

/// Sub-modes of the wizard modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectWizardMode {
    Form,
    PickingPreset {
        selected: usize,
    },
    PickingProtocol {
        selected: usize,
    },
    PickingModel {
        models: Vec<String>,
        selected: usize,
        filter: String,
    },
}

/// Hit target for mouse clicks.
#[derive(Debug, Clone, Copy)]
pub enum ConnectHitTarget {
    Field(Field),
    ToggleKeyMask,
    FetchModelsBtn,
    SubmitBtn,
    PresetChoice(usize),
    ProtocolChoice(usize),
    ModelChoice(usize),
}

#[derive(Debug, Clone)]
pub struct ConnectHitArea {
    pub rect: Rect,
    pub target: ConnectHitTarget,
}

/// Wizard state. Boxed by `ActiveModal::ConnectWizard`.
pub struct ConnectWizardState {
    pub window: ModalWindowState,
    pub preset_idx: usize,
    pub provider_name: String,
    pub protocol: ProtocolBackend,
    pub base_url: String,
    pub api_key: String,
    pub mask_api_key: bool,
    pub model_id: String,
    pub model_name: String,
    pub inject_think_tags: bool,

    pub focused: Field,
    pub mode: ConnectWizardMode,

    pub cursor_preset: usize,
    pub cursor_provider_name: usize,
    pub cursor_url: usize,
    pub cursor_key: usize,
    pub cursor_model: usize,
    pub cursor_model_name: usize,

    pub is_fetching: bool,
    pub fetch_status: Option<String>,
    pub fetched_models: Option<Vec<String>>,
    pub fetch_rx: Option<tokio::sync::oneshot::Receiver<Result<Vec<String>, String>>>,

    pub error: String,
    pub scroll_offset: usize,
    pub content_area: Option<Rect>,
    pub hit_areas: Vec<ConnectHitArea>,
}

impl std::fmt::Debug for ConnectWizardState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectWizardState")
            .field("preset_idx", &self.preset_idx)
            .field("provider_name", &self.provider_name)
            .field("protocol", &self.protocol)
            .field("base_url", &self.base_url)
            .field("model_id", &self.model_id)
            .field("focused", &self.focused)
            .field("mode", &self.mode)
            .finish()
    }
}

impl Clone for ConnectWizardState {
    fn clone(&self) -> Self {
        Self {
            window: ModalWindowState::new(),
            preset_idx: self.preset_idx,
            provider_name: self.provider_name.clone(),
            protocol: self.protocol,
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            mask_api_key: self.mask_api_key,
            model_id: self.model_id.clone(),
            model_name: self.model_name.clone(),
            inject_think_tags: self.inject_think_tags,
            focused: self.focused,
            mode: self.mode.clone(),
            cursor_preset: self.cursor_preset,
            cursor_provider_name: self.cursor_provider_name,
            cursor_url: self.cursor_url,
            cursor_key: self.cursor_key,
            cursor_model: self.cursor_model,
            cursor_model_name: self.cursor_model_name,
            is_fetching: self.is_fetching,
            fetch_status: self.fetch_status.clone(),
            fetched_models: self.fetched_models.clone(),
            fetch_rx: None,
            error: self.error.clone(),
            scroll_offset: self.scroll_offset,
            content_area: self.content_area,
            hit_areas: Vec::new(),
        }
    }
}

impl Default for ConnectWizardState {
    fn default() -> Self {
        let preset = PRESET_DEFS[0];
        Self {
            window: ModalWindowState::new(),
            preset_idx: 0,
            provider_name: preset.label.to_string(),
            protocol: preset.protocol,
            base_url: preset.base_url.to_string(),
            api_key: String::new(),
            mask_api_key: true,
            model_id: String::new(),
            model_name: String::new(),
            inject_think_tags: preset.injects_think_tags,
            focused: Field::Preset,
            mode: ConnectWizardMode::Form,
            cursor_preset: 0,
            cursor_provider_name: preset.label.len(),
            cursor_url: preset.base_url.len(),
            cursor_key: 0,
            cursor_model: 0,
            cursor_model_name: 0,
            is_fetching: false,
            fetch_status: None,
            fetched_models: None,
            fetch_rx: None,
            error: String::new(),
            scroll_offset: 0,
            content_area: None,
            hit_areas: Vec::new(),
        }
    }
}

impl ConnectWizardState {
    pub fn current_preset_id(&self) -> &'static str {
        PRESET_DEFS[self.preset_idx.min(PRESET_DEFS.len() - 1)].id
    }

    pub fn set_preset(&mut self, idx: usize) {
        let idx = idx.min(PRESET_DEFS.len() - 1);
        self.preset_idx = idx;
        let def = PRESET_DEFS[idx];
        self.provider_name = def.label.to_string();
        self.protocol = def.protocol;
        self.inject_think_tags = def.injects_think_tags;

        // If base_url was default or empty, prefill new default
        let prev_urls: Vec<&str> = PRESET_DEFS.iter().map(|p| p.base_url).collect();
        if self.base_url.is_empty() || prev_urls.contains(&self.base_url.as_str()) {
            self.base_url = def.base_url.to_string();
            self.cursor_url = self.base_url.len();
        }
        self.cursor_provider_name = self.provider_name.len();
        self.error.clear();
    }

    pub fn focus_next(&mut self) {
        self.focused = self.focused.next();
        self.error.clear();
    }

    pub fn focus_prev(&mut self) {
        self.focused = self.focused.prev();
        self.error.clear();
    }

    pub fn insert_char(&mut self, c: char) {
        self.error.clear();
        match self.focused {
            Field::ProviderName => {
                let idx = self.cursor_provider_name.min(self.provider_name.len());
                self.provider_name.insert(idx, c);
                self.cursor_provider_name = idx + c.len_utf8();
            }
            Field::Url => {
                let idx = self.cursor_url.min(self.base_url.len());
                self.base_url.insert(idx, c);
                self.cursor_url = idx + c.len_utf8();
            }
            Field::ApiKey => {
                let idx = self.cursor_key.min(self.api_key.len());
                self.api_key.insert(idx, c);
                self.cursor_key = idx + c.len_utf8();
            }
            Field::ModelId => {
                let idx = self.cursor_model.min(self.model_id.len());
                self.model_id.insert(idx, c);
                self.cursor_model = idx + c.len_utf8();
                if self.model_name.is_empty() {
                    self.model_name = self.model_id.clone();
                    self.cursor_model_name = self.model_name.len();
                }
            }
            Field::ModelName => {
                let idx = self.cursor_model_name.min(self.model_name.len());
                self.model_name.insert(idx, c);
                self.cursor_model_name = idx + c.len_utf8();
            }
            _ => {}
        }
    }

    pub fn backspace(&mut self) {
        self.error.clear();
        match self.focused {
            Field::ProviderName => {
                if self.cursor_provider_name > 0 && !self.provider_name.is_empty() {
                    let idx = self.cursor_provider_name.min(self.provider_name.len());
                    let prev_idx = self.provider_name[..idx]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.provider_name.remove(prev_idx);
                    self.cursor_provider_name = prev_idx;
                }
            }
            Field::Url => {
                if self.cursor_url > 0 && !self.base_url.is_empty() {
                    let idx = self.cursor_url.min(self.base_url.len());
                    let prev_idx = self.base_url[..idx]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.base_url.remove(prev_idx);
                    self.cursor_url = prev_idx;
                }
            }
            Field::ApiKey => {
                if self.cursor_key > 0 && !self.api_key.is_empty() {
                    let idx = self.cursor_key.min(self.api_key.len());
                    let prev_idx = self.api_key[..idx]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.api_key.remove(prev_idx);
                    self.cursor_key = prev_idx;
                }
            }
            Field::ModelId => {
                if self.cursor_model > 0 && !self.model_id.is_empty() {
                    let idx = self.cursor_model.min(self.model_id.len());
                    let prev_idx = self.model_id[..idx]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.model_id.remove(prev_idx);
                    self.cursor_model = prev_idx;
                }
            }
            Field::ModelName => {
                if self.cursor_model_name > 0 && !self.model_name.is_empty() {
                    let idx = self.cursor_model_name.min(self.model_name.len());
                    let prev_idx = self.model_name[..idx]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.model_name.remove(prev_idx);
                    self.cursor_model_name = prev_idx;
                }
            }
            _ => {}
        }
    }

    pub fn validate_and_build(&mut self) -> Option<ConnectWizardResult> {
        let provider = self.current_preset_id().to_string();
        let url = self.base_url.trim().to_string();
        let model_id = self.model_id.trim().to_string();
        let api_key = self.api_key.trim().to_string();
        let display_name = if self.model_name.trim().is_empty() {
            format!("{} · {}", self.provider_name.trim(), model_id)
        } else {
            self.model_name.trim().to_string()
        };

        if model_id.is_empty() {
            self.error = "Model ID is required.".to_string();
            self.focused = Field::ModelId;
            self.cursor_model = self.model_id.len();
            return None;
        }
        if provider == "custom" && url.is_empty() {
            self.error = "Base URL is required for custom provider.".to_string();
            self.focused = Field::Url;
            self.cursor_url = self.base_url.len();
            return None;
        }
        if !url.is_empty() && !url.starts_with("http://") && !url.starts_with("https://") {
            self.error = "Base URL must start with http:// or https://".to_string();
            self.focused = Field::Url;
            self.cursor_url = self.base_url.len();
            return None;
        }
        if api_key.is_empty() {
            self.error = "API key is required.".to_string();
            self.focused = Field::ApiKey;
            self.cursor_key = self.api_key.len();
            return None;
        }

        Some(ConnectWizardResult {
            provider,
            model_id,
            display_name,
            api_key,
            base_url: url,
            protocol: self.protocol,
            injects_think_tags: self.inject_think_tags,
        })
    }

    /// Check if background model fetch returned a result.
    pub fn poll_fetch_rx(&mut self) -> bool {
        if let Some(ref mut rx) = self.fetch_rx {
            if let Ok(res) = rx.try_recv() {
                self.is_fetching = false;
                self.fetch_rx = None;
                match res {
                    Ok(models) => {
                        let count = models.len();
                        self.fetched_models = Some(models.clone());
                        self.fetch_status = Some(format!("Fetched {count} models"));
                        self.mode = ConnectWizardMode::PickingModel {
                            models,
                            selected: 0,
                            filter: String::new(),
                        };
                        return true;
                    }
                    Err(e) => {
                        self.fetch_status = Some(format!("Error: {e}"));
                        self.error = e;
                        self.mode = ConnectWizardMode::Form;
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Trigger asynchronous upstream model fetch.
    pub fn start_fetch_models(&mut self) {
        if self.base_url.trim().is_empty() {
            self.error = "Please provide Base URL first to fetch models.".to_string();
            self.focused = Field::Url;
            return;
        }
        let url = self.base_url.trim().to_string();
        let key = self.api_key.trim().to_string();
        let proto = self.protocol;

        self.is_fetching = true;
        self.error.clear();
        self.fetch_status = Some("Fetching models from upstream API...".to_string());

        let (tx, rx) = tokio::sync::oneshot::channel();
        self.fetch_rx = Some(rx);

        tokio::spawn(async move {
            let res = fetch_upstream_models_async(&url, &key, proto).await;
            let _ = tx.send(res);
        });
    }
}

/// Fetch upstream model list from API.
async fn fetch_upstream_models_async(
    base_url: &str,
    api_key: &str,
    protocol: ProtocolBackend,
) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP Client error: {e}"))?;

    let trimmed_base = base_url.trim().trim_end_matches('/');
    let target_url = if trimmed_base.ends_with("/models") {
        trimmed_base.to_string()
    } else {
        format!("{trimmed_base}/models")
    };

    let mut req = client.get(&target_url);
    if !api_key.trim().is_empty() {
        if matches!(protocol, ProtocolBackend::Messages) {
            req = req.header("x-api-key", api_key.trim());
            req = req.header("anthropic-version", "2023-06-01");
        } else {
            req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
        }
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Server returned HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| format!("Parse error: {e}"))?;
    let mut model_ids = Vec::new();

    if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
        for item in data {
            if let Some(id) = item.get("id").and_then(|id| id.as_str()) {
                model_ids.push(id.to_string());
            }
        }
    } else if let Some(arr) = json.as_array() {
        for item in arr {
            if let Some(id) = item.get("id").and_then(|id| id.as_str()) {
                model_ids.push(id.to_string());
            } else if let Some(s) = item.as_str() {
                model_ids.push(s.to_string());
            }
        }
    }

    if model_ids.is_empty() {
        return Err("No models found in API response".to_string());
    }

    model_ids.sort();
    model_ids.dedup();
    Ok(model_ids)
}

/// Handle keyboard events for connect wizard.
pub fn handle_wizard_key(state: &mut ConnectWizardState, key: &KeyEvent) -> WizardOutcome {
    state.poll_fetch_rx();

    // Mode-specific handling
    match &mut state.mode {
        ConnectWizardMode::PickingPreset { selected } => match key.code {
            KeyCode::Esc => {
                state.mode = ConnectWizardMode::Form;
                return WizardOutcome::Changed;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if *selected > 0 {
                    *selected -= 1;
                }
                return WizardOutcome::Changed;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if *selected + 1 < PRESET_DEFS.len() {
                    *selected += 1;
                }
                return WizardOutcome::Changed;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let sel = *selected;
                state.set_preset(sel);
                state.mode = ConnectWizardMode::Form;
                return WizardOutcome::Changed;
            }
            _ => return WizardOutcome::Unhandled,
        },
        ConnectWizardMode::PickingProtocol { selected } => match key.code {
            KeyCode::Esc => {
                state.mode = ConnectWizardMode::Form;
                return WizardOutcome::Changed;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if *selected > 0 {
                    *selected -= 1;
                }
                return WizardOutcome::Changed;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if *selected + 1 < ProtocolBackend::ALL.len() {
                    *selected += 1;
                }
                return WizardOutcome::Changed;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let sel = *selected;
                state.protocol = ProtocolBackend::ALL[sel];
                state.mode = ConnectWizardMode::Form;
                return WizardOutcome::Changed;
            }
            _ => return WizardOutcome::Unhandled,
        },
        ConnectWizardMode::PickingModel {
            models,
            selected,
            filter,
        } => match key.code {
            KeyCode::Esc => {
                state.mode = ConnectWizardMode::Form;
                return WizardOutcome::Changed;
            }
            KeyCode::Up => {
                if *selected > 0 {
                    *selected -= 1;
                }
                return WizardOutcome::Changed;
            }
            KeyCode::Down => {
                let filtered_count = models
                    .iter()
                    .filter(|m| {
                        filter.is_empty() || m.to_lowercase().contains(&filter.to_lowercase())
                    })
                    .count();
                if filtered_count > 0 && *selected + 1 < filtered_count {
                    *selected += 1;
                }
                return WizardOutcome::Changed;
            }
            KeyCode::Backspace => {
                filter.pop();
                *selected = 0;
                return WizardOutcome::Changed;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                filter.push(c);
                *selected = 0;
                return WizardOutcome::Changed;
            }
            KeyCode::Enter => {
                let filtered: Vec<String> = models
                    .iter()
                    .filter(|m| {
                        filter.is_empty() || m.to_lowercase().contains(&filter.to_lowercase())
                    })
                    .cloned()
                    .collect();
                if let Some(picked) = filtered.get(*selected) {
                    state.model_id = picked.clone();
                    state.cursor_model = state.model_id.len();
                    if state.model_name.is_empty() || state.model_name == state.model_id {
                        state.model_name = state.model_id.clone();
                        state.cursor_model_name = state.model_name.len();
                    }
                }
                state.mode = ConnectWizardMode::Form;
                return WizardOutcome::Changed;
            }
            _ => return WizardOutcome::Unhandled,
        },
        ConnectWizardMode::Form => {}
    }

    // Ctrl+S or Ctrl+Enter submits from anywhere
    if (key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL))
        || (key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        if let Some(result) = state.validate_and_build() {
            return WizardOutcome::Submitted(result);
        }
        return WizardOutcome::Changed;
    }

    match key.code {
        KeyCode::Esc => WizardOutcome::Closed,
        KeyCode::Tab => {
            state.focus_next();
            WizardOutcome::Changed
        }
        KeyCode::BackTab => {
            state.focus_prev();
            WizardOutcome::Changed
        }
        KeyCode::Up => {
            match state.focused {
                Field::Preset => {
                    if state.preset_idx > 0 {
                        state.set_preset(state.preset_idx - 1);
                    }
                }
                _ => state.focus_prev(),
            }
            WizardOutcome::Changed
        }
        KeyCode::Down => {
            match state.focused {
                Field::Preset => {
                    if state.preset_idx + 1 < PRESET_DEFS.len() {
                        state.set_preset(state.preset_idx + 1);
                    }
                }
                _ => state.focus_next(),
            }
            WizardOutcome::Changed
        }
        KeyCode::Left => {
            match state.focused {
                Field::Preset => {
                    if state.preset_idx > 0 {
                        state.set_preset(state.preset_idx - 1);
                    }
                }
                Field::Protocol => {
                    let idx = ProtocolBackend::ALL
                        .iter()
                        .position(|p| *p == state.protocol)
                        .unwrap_or(0);
                    if idx > 0 {
                        state.protocol = ProtocolBackend::ALL[idx - 1];
                    }
                }
                Field::InjectThinkTags => {
                    state.inject_think_tags = !state.inject_think_tags;
                }
                Field::ProviderName => {
                    state.cursor_provider_name = state.cursor_provider_name.saturating_sub(1);
                }
                Field::Url => {
                    state.cursor_url = state.cursor_url.saturating_sub(1);
                }
                Field::ApiKey => {
                    state.cursor_key = state.cursor_key.saturating_sub(1);
                }
                Field::ModelId => {
                    state.cursor_model = state.cursor_model.saturating_sub(1);
                }
                Field::ModelName => {
                    state.cursor_model_name = state.cursor_model_name.saturating_sub(1);
                }
                _ => {}
            }
            WizardOutcome::Changed
        }
        KeyCode::Right => {
            match state.focused {
                Field::Preset => {
                    if state.preset_idx + 1 < PRESET_DEFS.len() {
                        state.set_preset(state.preset_idx + 1);
                    }
                }
                Field::Protocol => {
                    let idx = ProtocolBackend::ALL
                        .iter()
                        .position(|p| *p == state.protocol)
                        .unwrap_or(0);
                    if idx + 1 < ProtocolBackend::ALL.len() {
                        state.protocol = ProtocolBackend::ALL[idx + 1];
                    }
                }
                Field::InjectThinkTags => {
                    state.inject_think_tags = !state.inject_think_tags;
                }
                Field::ProviderName => {
                    if state.cursor_provider_name < state.provider_name.len() {
                        state.cursor_provider_name += 1;
                    }
                }
                Field::Url => {
                    if state.cursor_url < state.base_url.len() {
                        state.cursor_url += 1;
                    }
                }
                Field::ApiKey => {
                    if state.cursor_key < state.api_key.len() {
                        state.cursor_key += 1;
                    }
                }
                Field::ModelId => {
                    if state.cursor_model < state.model_id.len() {
                        state.cursor_model += 1;
                    }
                }
                Field::ModelName => {
                    if state.cursor_model_name < state.model_name.len() {
                        state.cursor_model_name += 1;
                    }
                }
                _ => {}
            }
            WizardOutcome::Changed
        }
        KeyCode::Home => {
            match state.focused {
                Field::ProviderName => state.cursor_provider_name = 0,
                Field::Url => state.cursor_url = 0,
                Field::ApiKey => state.cursor_key = 0,
                Field::ModelId => state.cursor_model = 0,
                Field::ModelName => state.cursor_model_name = 0,
                _ => {}
            }
            WizardOutcome::Changed
        }
        KeyCode::End => {
            match state.focused {
                Field::ProviderName => state.cursor_provider_name = state.provider_name.len(),
                Field::Url => state.cursor_url = state.base_url.len(),
                Field::ApiKey => state.cursor_key = state.api_key.len(),
                Field::ModelId => state.cursor_model = state.model_id.len(),
                Field::ModelName => state.cursor_model_name = state.model_name.len(),
                _ => {}
            }
            WizardOutcome::Changed
        }
        KeyCode::Backspace => {
            state.backspace();
            WizardOutcome::Changed
        }
        KeyCode::Enter => {
            match state.focused {
                Field::Preset => {
                    state.mode = ConnectWizardMode::PickingPreset {
                        selected: state.preset_idx,
                    };
                    WizardOutcome::Changed
                }
                Field::Protocol => {
                    let idx = ProtocolBackend::ALL
                        .iter()
                        .position(|p| *p == state.protocol)
                        .unwrap_or(0);
                    state.mode = ConnectWizardMode::PickingProtocol { selected: idx };
                    WizardOutcome::Changed
                }
                Field::FetchModels => {
                    state.start_fetch_models();
                    WizardOutcome::Changed
                }
                Field::InjectThinkTags => {
                    state.inject_think_tags = !state.inject_think_tags;
                    WizardOutcome::Changed
                }
                Field::Submit => {
                    if let Some(result) = state.validate_and_build() {
                        WizardOutcome::Submitted(result)
                    } else {
                        WizardOutcome::Changed
                    }
                }
                _ => {
                    // Enter in text field validates and submits if all valid
                    if let Some(result) = state.validate_and_build() {
                        WizardOutcome::Submitted(result)
                    } else {
                        WizardOutcome::Changed
                    }
                }
            }
        }
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                return WizardOutcome::Unhandled;
            }
            // Vim j/k navigation when focused on Preset field and unedited
            if state.focused == Field::Preset {
                if c == 'j' {
                    if state.preset_idx + 1 < PRESET_DEFS.len() {
                        state.set_preset(state.preset_idx + 1);
                    }
                    return WizardOutcome::Changed;
                } else if c == 'k' {
                    if state.preset_idx > 0 {
                        state.set_preset(state.preset_idx - 1);
                    }
                    return WizardOutcome::Changed;
                }
            }
            state.insert_char(c);
            WizardOutcome::Changed
        }
        _ => WizardOutcome::Unhandled,
    }
}

/// Handle mouse events for connect wizard.
pub fn handle_wizard_mouse(
    state: &mut ConnectWizardState,
    kind: MouseEventKind,
    col: u16,
    row: u16,
) -> WizardOutcome {
    state.poll_fetch_rx();

    // Check modal chrome mouse outcome (close button, tab click)
    let outcome = mw::handle_modal_mouse(&mut state.window, kind, col, row);
    if outcome == mw::ModalWindowOutcome::CloseRequested {
        return WizardOutcome::Closed;
    }

    match kind {
        MouseEventKind::ScrollDown => {
            state.scroll_offset = state.scroll_offset.saturating_add(2);
            return WizardOutcome::Changed;
        }
        MouseEventKind::ScrollUp => {
            state.scroll_offset = state.scroll_offset.saturating_sub(2);
            return WizardOutcome::Changed;
        }
        MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
            for hit in &state.hit_areas {
                if col >= hit.rect.x
                    && col < hit.rect.x + hit.rect.width
                    && row >= hit.rect.y
                    && row < hit.rect.y + hit.rect.height
                {
                    match hit.target {
                        ConnectHitTarget::Field(f) => {
                            state.focused = f;
                            state.error.clear();
                            if f == Field::Preset {
                                state.mode = ConnectWizardMode::PickingPreset {
                                    selected: state.preset_idx,
                                };
                            } else if f == Field::Protocol {
                                let idx = ProtocolBackend::ALL
                                    .iter()
                                    .position(|p| *p == state.protocol)
                                    .unwrap_or(0);
                                state.mode = ConnectWizardMode::PickingProtocol { selected: idx };
                            } else if f == Field::InjectThinkTags {
                                state.inject_think_tags = !state.inject_think_tags;
                            }
                            return WizardOutcome::Changed;
                        }
                        ConnectHitTarget::ToggleKeyMask => {
                            state.mask_api_key = !state.mask_api_key;
                            return WizardOutcome::Changed;
                        }
                        ConnectHitTarget::FetchModelsBtn => {
                            state.start_fetch_models();
                            return WizardOutcome::Changed;
                        }
                        ConnectHitTarget::SubmitBtn => {
                            if let Some(res) = state.validate_and_build() {
                                return WizardOutcome::Submitted(res);
                            }
                            return WizardOutcome::Changed;
                        }
                        ConnectHitTarget::PresetChoice(idx) => {
                            state.set_preset(idx);
                            state.mode = ConnectWizardMode::Form;
                            return WizardOutcome::Changed;
                        }
                        ConnectHitTarget::ProtocolChoice(idx) => {
                            if idx < ProtocolBackend::ALL.len() {
                                state.protocol = ProtocolBackend::ALL[idx];
                            }
                            state.mode = ConnectWizardMode::Form;
                            return WizardOutcome::Changed;
                        }
                        ConnectHitTarget::ModelChoice(idx) => {
                            if let ConnectWizardMode::PickingModel {
                                ref models,
                                ref filter,
                                ..
                            } = state.mode
                            {
                                let filtered: Vec<&String> = models
                                    .iter()
                                    .filter(|m| {
                                        filter.is_empty()
                                            || m.to_lowercase().contains(&filter.to_lowercase())
                                    })
                                    .collect();
                                if let Some(m) = filtered.get(idx) {
                                    state.model_id = (*m).clone();
                                    state.cursor_model = state.model_id.len();
                                    if state.model_name.is_empty()
                                        || state.model_name == state.model_id
                                    {
                                        state.model_name = state.model_id.clone();
                                        state.cursor_model_name = state.model_name.len();
                                    }
                                }
                            }
                            state.mode = ConnectWizardMode::Form;
                            return WizardOutcome::Changed;
                        }
                    }
                }
            }
        }
        _ => {}
    }

    WizardOutcome::Unhandled
}

/// Render the connect wizard modal.
pub fn render_wizard(
    buf: &mut Buffer,
    area: Rect,
    state: &mut ConnectWizardState,
    theme: &Theme,
    compact: bool,
) {
    state.poll_fetch_rx();
    state.hit_areas.clear();

    let shortcuts = [
        Shortcut {
            label: "↑/↓ nav",
            clickable: false,
            id: 0,
        },
        Shortcut {
            label: "Enter select/edit",
            clickable: false,
            id: 0,
        },
        Shortcut {
            label: "Ctrl+S connect",
            clickable: true,
            id: 1,
        },
        Shortcut {
            label: "Esc close",
            clickable: false,
            id: 0,
        },
    ];

    let modal_config = ModalWindowConfig {
        title: "Connect Model Provider",
        tabs: None,
        shortcuts: &shortcuts,
        sizing: ModalSizing {
            width_pct: 0.85,
            max_width: 110,
            min_width: 50,
            v_margin: 2,
            h_pad: 2,
            v_pad: 1,
            footer_lines: 2,
        }
        .with_compact(compact),
        fold_info: None,
    };

    let Some(mca) = mw::render_modal_window(buf, area, &mut state.window, &modal_config, theme)
    else {
        return;
    };

    let content_area = mca.content;
    state.content_area = Some(content_area);

    // Clear content area rows with theme background
    let bg_style = Style::default().bg(theme.bg_base);
    for row_idx in 0..content_area.height {
        let y = content_area.y + row_idx;
        for x in content_area.x..content_area.x + content_area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_style(bg_style);
            }
        }
    }

    let mode = state.mode.clone();
    match mode {
        ConnectWizardMode::Form => {
            render_connect_form(buf, content_area, state, theme);
        }
        ConnectWizardMode::PickingPreset { selected } => {
            render_preset_picker(buf, content_area, selected, state, theme);
        }
        ConnectWizardMode::PickingProtocol { selected } => {
            render_protocol_picker(buf, content_area, selected, state, theme);
        }
        ConnectWizardMode::PickingModel {
            models,
            selected,
            filter,
        } => {
            render_model_picker(buf, content_area, &models, selected, &filter, state, theme);
        }
    }
}

/// Render the main interactive connect form.
fn render_connect_form(
    buf: &mut Buffer,
    area: Rect,
    state: &mut ConnectWizardState,
    theme: &Theme,
) {
    let mut cur_y = area.y;
    let max_y = area.y + area.height;
    let label_width: u16 = 22;

    // Section Header
    if cur_y < max_y {
        let header = Line::from(vec![Span::styled(
            "◆ Provider & Endpoint Configuration",
            Style::default()
                .fg(theme.fuzzy_accent)
                .add_modifier(Modifier::BOLD),
        )]);
        buf.set_line(area.x, cur_y, &header, area.width);
        cur_y += 2;
    }

    // List of form rows
    let rows = [
        (
            Field::Preset,
            "Provider Preset",
            state.current_preset_id(),
            format!("[ {} ▾ ]", PRESET_DEFS[state.preset_idx].label),
        ),
        (
            Field::ProviderName,
            "Provider Name",
            &state.provider_name,
            state.provider_name.clone(),
        ),
        (
            Field::Protocol,
            "Protocol Format",
            state.protocol.as_str(),
            format!("[ {} ▾ ]", state.protocol.display_label()),
        ),
        (
            Field::Url,
            "Base URL",
            &state.base_url,
            if state.base_url.is_empty() {
                "<None / Enter Base URL>".to_string()
            } else {
                state.base_url.clone()
            },
        ),
        (
            Field::ApiKey,
            "API Key",
            &state.api_key,
            if state.api_key.is_empty() {
                "<Enter API Key>".to_string()
            } else if state.mask_api_key {
                "•".repeat(state.api_key.len().min(24))
            } else {
                state.api_key.clone()
            },
        ),
        (
            Field::ModelId,
            "Model ID",
            &state.model_id,
            if state.model_id.is_empty() {
                "<Enter Model ID or Auto-Fetch below>".to_string()
            } else {
                state.model_id.clone()
            },
        ),
        (
            Field::FetchModels,
            "Auto-Fetch Models",
            "",
            if state.is_fetching {
                "⟳ Fetching models from upstream...".to_string()
            } else {
                "[ ⟳ Fetch Upstream Models ]".to_string()
            },
        ),
        (
            Field::ModelName,
            "Model Display Name",
            &state.model_name,
            if state.model_name.is_empty() {
                "<Optional Display Name>".to_string()
            } else {
                state.model_name.clone()
            },
        ),
        (
            Field::InjectThinkTags,
            "Think Tag Injection",
            "",
            if state.inject_think_tags {
                "[ ✓ Enabled (<think> tags) ]".to_string()
            } else {
                "[   Disabled ]".to_string()
            },
        ),
    ];

    for (field, label, raw_val, display_val) in rows {
        if cur_y >= max_y {
            break;
        }

        let is_focused = state.focused == field;
        let row_rect = Rect {
            x: area.x,
            y: cur_y,
            width: area.width,
            height: 1,
        };

        // Record hit area for row click
        state.hit_areas.push(ConnectHitArea {
            rect: row_rect,
            target: ConnectHitTarget::Field(field),
        });

        if is_focused {
            let row_bg = Style::default().bg(theme.bg_highlight);
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, cur_y)) {
                    cell.set_style(row_bg);
                }
            }
        }

        let indicator = if is_focused { "▶ " } else { "  " };
        let label_style = if is_focused {
            Style::default()
                .fg(theme.fuzzy_accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_primary)
        };

        let mut spans = vec![
            Span::styled(indicator, Style::default().fg(theme.fuzzy_accent)),
            Span::styled(format!("{label:<20}"), label_style),
            Span::styled(" │ ", Style::default().fg(theme.gray_dim)),
        ];

        if field == Field::FetchModels {
            let btn_style = if is_focused {
                Style::default()
                    .fg(theme.accent_success)
                    .bg(theme.bg_base)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.fuzzy_accent)
                    .add_modifier(Modifier::BOLD)
            };
            spans.push(Span::styled(&display_val, btn_style));
        } else if field == Field::ApiKey {
            let val_style = if raw_val.is_empty() {
                Style::default().fg(theme.gray)
            } else if is_focused {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_primary)
            };
            spans.push(Span::styled(&display_val, val_style));

            // Show mask toggle button
            let mask_btn_text = if state.mask_api_key {
                " [Show]"
            } else {
                " [Hide]"
            };
            let mask_btn_style = Style::default().fg(theme.fuzzy_accent);
            let mask_x = area.x + label_width + display_val.len() as u16 + 5;
            if mask_x + 8 < area.x + area.width {
                state.hit_areas.push(ConnectHitArea {
                    rect: Rect {
                        x: mask_x,
                        y: cur_y,
                        width: 8,
                        height: 1,
                    },
                    target: ConnectHitTarget::ToggleKeyMask,
                });
                spans.push(Span::styled(mask_btn_text, mask_btn_style));
            }
        } else if field == Field::Preset
            || field == Field::Protocol
            || field == Field::InjectThinkTags
        {
            let chip_style = if is_focused {
                Style::default()
                    .fg(theme.fuzzy_accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_primary)
            };
            spans.push(Span::styled(&display_val, chip_style));
        } else {
            let is_empty = raw_val.is_empty();
            let val_style = if is_empty {
                Style::default().fg(theme.gray)
            } else if is_focused {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_primary)
            };

            if is_focused && !is_empty {
                // Render with block cursor
                let cursor = match field {
                    Field::ProviderName => state.cursor_provider_name,
                    Field::Url => state.cursor_url,
                    Field::ModelId => state.cursor_model,
                    Field::ModelName => state.cursor_model_name,
                    _ => display_val.len(),
                }
                .min(display_val.len());

                spans.push(Span::styled(&display_val[..cursor], val_style));
                spans.push(Span::styled(
                    "█",
                    Style::default()
                        .fg(theme.fuzzy_accent)
                        .add_modifier(Modifier::SLOW_BLINK),
                ));
                spans.push(Span::styled(&display_val[cursor..], val_style));
            } else {
                spans.push(Span::styled(&display_val, val_style));
            }
        }

        buf.set_line(area.x, cur_y, &Line::from(spans), area.width);
        cur_y += 1;
    }

    cur_y += 1;

    // Error or status bar
    if !state.error.is_empty() && cur_y < max_y {
        let err_line = Line::from(vec![
            Span::styled(
                "✗ ",
                Style::default()
                    .fg(theme.accent_error)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(&state.error, Style::default().fg(theme.accent_error)),
        ]);
        buf.set_line(area.x + 2, cur_y, &err_line, area.width.saturating_sub(4));
        cur_y += 1;
    } else if let Some(ref st) = state.fetch_status {
        if cur_y < max_y {
            let status_line = Line::from(vec![
                Span::styled("ℹ ", Style::default().fg(theme.fuzzy_accent)),
                Span::styled(st, Style::default().fg(theme.fuzzy_accent)),
            ]);
            buf.set_line(
                area.x + 2,
                cur_y,
                &status_line,
                area.width.saturating_sub(4),
            );
            cur_y += 1;
        }
    }

    cur_y += 1;

    // Submit / Connect button
    if cur_y < max_y {
        let is_submit_focused = state.focused == Field::Submit;
        let submit_btn_style = if is_submit_focused {
            Style::default()
                .bg(theme.fuzzy_accent)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .bg(theme.bg_highlight)
                .fg(theme.fuzzy_accent)
                .add_modifier(Modifier::BOLD)
        };

        let btn_text = "  [ Connect & Save Model ]  ";
        let btn_x = area.x + 4;
        let btn_w = btn_text.len() as u16;

        state.hit_areas.push(ConnectHitArea {
            rect: Rect {
                x: btn_x,
                y: cur_y,
                width: btn_w,
                height: 1,
            },
            target: ConnectHitTarget::SubmitBtn,
        });

        buf.set_string(btn_x, cur_y, btn_text, submit_btn_style);
    }
}

/// Render the preset chooser dropdown.
fn render_preset_picker(
    buf: &mut Buffer,
    area: Rect,
    selected: usize,
    state: &mut ConnectWizardState,
    theme: &Theme,
) {
    let mut cur_y = area.y;
    let max_y = area.y + area.height;

    let header = Line::from(vec![Span::styled(
        "◆ Select Provider Preset (↑/↓ to navigate, Enter to choose, Esc back)",
        Style::default()
            .fg(theme.fuzzy_accent)
            .add_modifier(Modifier::BOLD),
    )]);
    buf.set_line(area.x, cur_y, &header, area.width);
    cur_y += 2;

    for (i, def) in PRESET_DEFS.iter().enumerate() {
        if cur_y >= max_y {
            break;
        }
        let is_sel = i == selected;
        let row_rect = Rect {
            x: area.x,
            y: cur_y,
            width: area.width,
            height: 1,
        };
        state.hit_areas.push(ConnectHitArea {
            rect: row_rect,
            target: ConnectHitTarget::PresetChoice(i),
        });

        if is_sel {
            let highlight = Style::default().bg(theme.bg_highlight);
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, cur_y)) {
                    cell.set_style(highlight);
                }
            }
        }

        let pfx = if is_sel { " ▶ " } else { "   " };
        let pfx_style = Style::default()
            .fg(theme.fuzzy_accent)
            .add_modifier(Modifier::BOLD);
        let name_style = if is_sel {
            Style::default()
                .fg(theme.fuzzy_accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_primary)
        };
        let url_style = Style::default().fg(theme.gray);

        let spans = vec![
            Span::styled(pfx, pfx_style),
            Span::styled(format!("{:<28}", def.label), name_style),
            Span::styled(
                format!(" ({})  {}", def.protocol.short_label(), def.base_url),
                url_style,
            ),
        ];

        buf.set_line(area.x, cur_y, &Line::from(spans), area.width);
        cur_y += 1;
    }
}

/// Render the protocol format picker.
fn render_protocol_picker(
    buf: &mut Buffer,
    area: Rect,
    selected: usize,
    state: &mut ConnectWizardState,
    theme: &Theme,
) {
    let mut cur_y = area.y;
    let max_y = area.y + area.height;

    let header = Line::from(vec![Span::styled(
        "◆ Select Protocol Format (↑/↓ to navigate, Enter to choose, Esc back)",
        Style::default()
            .fg(theme.fuzzy_accent)
            .add_modifier(Modifier::BOLD),
    )]);
    buf.set_line(area.x, cur_y, &header, area.width);
    cur_y += 2;

    for (i, proto) in ProtocolBackend::ALL.iter().enumerate() {
        if cur_y >= max_y {
            break;
        }
        let is_sel = i == selected;
        let row_rect = Rect {
            x: area.x,
            y: cur_y,
            width: area.width,
            height: 1,
        };
        state.hit_areas.push(ConnectHitArea {
            rect: row_rect,
            target: ConnectHitTarget::ProtocolChoice(i),
        });

        if is_sel {
            let highlight = Style::default().bg(theme.bg_highlight);
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, cur_y)) {
                    cell.set_style(highlight);
                }
            }
        }

        let pfx = if is_sel { " ▶ " } else { "   " };
        let spans = vec![
            Span::styled(
                pfx,
                Style::default()
                    .fg(theme.fuzzy_accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                proto.display_label(),
                if is_sel {
                    Style::default()
                        .fg(theme.fuzzy_accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text_primary)
                },
            ),
        ];

        buf.set_line(area.x, cur_y, &Line::from(spans), area.width);
        cur_y += 1;
    }
}

/// Render the fetched upstream model picker.
fn render_model_picker(
    buf: &mut Buffer,
    area: Rect,
    models: &[String],
    selected: usize,
    filter: &str,
    state: &mut ConnectWizardState,
    theme: &Theme,
) {
    let mut cur_y = area.y;
    let max_y = area.y + area.height;

    let header = Line::from(vec![Span::styled(
        "◆ Pick Model ID from Upstream Server (Type to filter, Enter to select, Esc back)",
        Style::default()
            .fg(theme.fuzzy_accent)
            .add_modifier(Modifier::BOLD),
    )]);
    buf.set_line(area.x, cur_y, &header, area.width);
    cur_y += 1;

    let filter_line = Line::from(vec![
        Span::styled(" Filter: ", Style::default().fg(theme.gray_dim)),
        Span::styled(
            if filter.is_empty() {
                "(all models)"
            } else {
                filter
            },
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    buf.set_line(area.x, cur_y, &filter_line, area.width);
    cur_y += 2;

    let filtered: Vec<&String> = models
        .iter()
        .filter(|m| filter.is_empty() || m.to_lowercase().contains(&filter.to_lowercase()))
        .collect();

    if filtered.is_empty() {
        let none_line = Line::from(vec![Span::styled(
            "   (No models matching filter)",
            Style::default().fg(theme.gray),
        )]);
        buf.set_line(area.x, cur_y, &none_line, area.width);
        return;
    }

    // Scroll viewport window
    let visible_rows = (max_y.saturating_sub(cur_y)) as usize;
    let start_idx = if selected >= visible_rows {
        selected + 1 - visible_rows
    } else {
        0
    };

    for (i, &model_id) in filtered
        .iter()
        .enumerate()
        .skip(start_idx)
        .take(visible_rows)
    {
        if cur_y >= max_y {
            break;
        }
        let is_sel = i == selected;
        let row_rect = Rect {
            x: area.x,
            y: cur_y,
            width: area.width,
            height: 1,
        };
        state.hit_areas.push(ConnectHitArea {
            rect: row_rect,
            target: ConnectHitTarget::ModelChoice(i),
        });

        if is_sel {
            let highlight = Style::default().bg(theme.bg_highlight);
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, cur_y)) {
                    cell.set_style(highlight);
                }
            }
        }

        let pfx = if is_sel { " ▶ " } else { "   " };
        let spans = vec![
            Span::styled(
                pfx,
                Style::default()
                    .fg(theme.fuzzy_accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                model_id,
                if is_sel {
                    Style::default()
                        .fg(theme.fuzzy_accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text_primary)
                },
            ),
        ];

        buf.set_line(area.x, cur_y, &Line::from(spans), area.width);
        cur_y += 1;
    }
}

/// Legacy render function signature for compatibility.
pub fn render_wizard_legacy(buf: &mut Buffer, area: Rect, state: &mut ConnectWizardState) {
    let theme = Theme::current();
    render_wizard(buf, area, state, &theme, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn kc(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn default_state_uses_first_preset_and_prefilled_url() {
        let s = ConnectWizardState::default();
        assert_eq!(s.preset_idx, 0);
        assert_eq!(s.base_url, "https://api.openai.com/v1");
        assert_eq!(s.focused, Field::Preset);
        assert!(s.error.is_empty());
    }

    #[test]
    fn switching_preset_updates_url_when_url_was_unedited() {
        let mut s = ConnectWizardState::default();
        s.set_preset(3); // topodrive
        assert_eq!(s.current_preset_id(), "topodrive");
        assert_eq!(s.base_url, "https://api.topodrive.top/v1");
    }

    #[test]
    fn custom_preset_clears_url() {
        let mut s = ConnectWizardState::default();
        s.set_preset(9); // custom
        assert_eq!(s.current_preset_id(), "custom");
        assert_eq!(s.base_url, "");
    }

    #[test]
    fn validation_requires_model_id() {
        let mut s = ConnectWizardState::default();
        let res = s.validate_and_build();
        assert!(res.is_none());
        assert!(s.error.contains("Model ID"));
        assert_eq!(s.focused, Field::ModelId);
    }

    #[test]
    fn validation_requires_api_key() {
        let mut s = ConnectWizardState::default();
        s.model_id = "m".into();
        let res = s.validate_and_build();
        assert!(res.is_none());
        assert!(s.error.contains("API key"));
        assert_eq!(s.focused, Field::ApiKey);
    }

    #[test]
    fn validation_passes_for_openai_preset() {
        let mut s = ConnectWizardState::default();
        s.model_id = "gpt-5.6-luna".into();
        s.api_key = "sk-test".into();
        let res = s.validate_and_build().expect("valid");
        assert_eq!(res.provider, "openai");
        assert_eq!(res.model_id, "gpt-5.6-luna");
        assert_eq!(res.base_url, "https://api.openai.com/v1");
        assert!(!res.injects_think_tags);
    }

    #[test]
    fn minimax_preset_sets_injects_think_tags() {
        let mut s = ConnectWizardState::default();
        s.set_preset(7); // minimax_cn
        s.model_id = "MiniMax-M3".into();
        s.api_key = "sk-mm".into();
        let res = s.validate_and_build().expect("valid");
        assert_eq!(res.provider, "minimax_cn");
        assert!(res.injects_think_tags);
    }

    #[test]
    fn tab_cycles_through_fields() {
        let mut s = ConnectWizardState::default();
        assert_eq!(s.focused, Field::Preset);
        s.focus_next();
        assert_eq!(s.focused, Field::ProviderName);
        s.focus_next();
        assert_eq!(s.focused, Field::Protocol);
    }

    #[test]
    fn typing_inserts_chars_at_cursor() {
        let mut s = ConnectWizardState::default();
        s.focused = Field::ModelId;
        s.insert_char('a');
        s.insert_char('b');
        s.insert_char('c');
        assert_eq!(s.model_id, "abc");
        assert_eq!(s.cursor_model, 3);
    }

    #[test]
    fn backspace_removes_previous_char() {
        let mut s = ConnectWizardState::default();
        s.focused = Field::ModelId;
        s.insert_char('a');
        s.insert_char('b');
        s.backspace();
        assert_eq!(s.model_id, "a");
        assert_eq!(s.cursor_model, 1);
    }

    #[test]
    fn esc_closes() {
        let mut s = ConnectWizardState::default();
        let out = handle_wizard_key(&mut s, &k(KeyCode::Esc));
        assert_eq!(out, WizardOutcome::Closed);
    }

    #[test]
    fn enter_with_valid_form_submits() {
        let mut s = ConnectWizardState::default();
        s.focused = Field::Submit;
        s.model_id = "m".into();
        s.api_key = "sk".into();
        let out = handle_wizard_key(&mut s, &k(KeyCode::Enter));
        match out {
            WizardOutcome::Submitted(r) => {
                assert_eq!(r.model_id, "m");
                assert_eq!(r.api_key, "sk");
                assert_eq!(r.provider, "openai");
            }
            other => panic!("expected Submitted, got {other:?}"),
        }
    }

    #[test]
    fn enter_in_text_field_submits_when_valid() {
        let mut s = ConnectWizardState::default();
        s.focused = Field::ApiKey;
        s.model_id = "m".into();
        s.api_key = "sk".into();
        let out = handle_wizard_key(&mut s, &k(KeyCode::Enter));
        match out {
            WizardOutcome::Submitted(r) => {
                assert_eq!(r.model_id, "m");
                assert_eq!(r.api_key, "sk");
            }
            other => panic!("expected Submitted, got {other:?}"),
        }
    }

    #[test]
    fn mouse_scroll_changes_scroll_offset() {
        let mut s = ConnectWizardState::default();
        let out = handle_wizard_mouse(&mut s, MouseEventKind::ScrollDown, 10, 10);
        assert_eq!(out, WizardOutcome::Changed);
        assert_eq!(s.scroll_offset, 2);

        let out = handle_wizard_mouse(&mut s, MouseEventKind::ScrollUp, 10, 10);
        assert_eq!(out, WizardOutcome::Changed);
        assert_eq!(s.scroll_offset, 0);
    }
}
