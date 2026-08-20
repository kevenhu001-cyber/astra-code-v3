//! Dispatch handler for `Action::OpenConnectWizard`. Pushes a
//! `ConnectWizard` modal onto the active agent's `active_modal` stack so
//! the existing modal-dispatch path picks it up.
//!
//! The companion [`dispatch_connect_wizard_submission`] consumes a
//! completed `ConnectWizardResult` and returns the same
//! `Effect::PersistConfig` (or equivalent) the one-shot
//! `Action::ConnectCustomModel` path produces. The modal dispatcher calls
//! this when the user presses Enter and validation passes.

use crate::app::actions::Effect;
use crate::app::app_view::{ActiveView, AppView};
use crate::views::connect_wizard::{ConnectWizardResult, ConnectWizardState};
use crate::views::modal::ActiveModal;

/// Push the `/connect` wizard modal onto the active agent. If no agent view
/// is active, the call is a no-op — `/connect` is session-scoped so the
/// caller (slash command) should already have routed to one.
pub(super) fn dispatch_open_connect_wizard(app: &mut AppView) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    // Replace any existing connect wizard (re-running /connect is a no-op
    // visually; it just re-opens the wizard at default state).
    agent.active_modal = Some(ActiveModal::ConnectWizard {
        state: Box::new(ConnectWizardState::default()),
    });
    vec![]
}

/// Run the dispatch path that the slash command would have triggered for
/// an equivalent `Action::ConnectCustomModel`. Same effect list — the
/// persistence layer can't tell the wizard submission from a typed
/// `/connect <preset> <model> <key>`.
pub(super) fn dispatch_connect_wizard_submission(
    app: &mut AppView,
    result: ConnectWizardResult,
) -> Vec<Effect> {
    use crate::app::actions::Action;

    let display_name = format!("{} \u00b7 {}", result.provider, result.model_id);
    crate::app::dispatch::router::dispatch(
        Action::ConnectCustomModel {
            provider: result.provider,
            model_id: result.model_id,
            display_name,
            api_key: result.api_key,
            base_url: result.base_url,
            injects_think_tags: result.injects_think_tags,
        },
        app,
    )
}