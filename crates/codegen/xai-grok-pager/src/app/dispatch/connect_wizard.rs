//! Dispatch handler for `Action::OpenConnectWizard`. Pushes a
//! `ConnectWizard` modal onto the active agent's `active_modal` stack so
//! the existing modal-dispatch path picks it up. The actual submission
//! path lives in `app/modals.rs` (the `ActiveModal::ConnectWizard` arm)
//! so the dispatcher can close the modal before emitting the
//! `Action::ConnectCustomModel`.

use crate::app::app_view::{ActiveView, AppView};
use crate::views::connect_wizard::ConnectWizardState;
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