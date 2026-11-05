//! Dispatch handler for `Action::OpenConnectWizard`. Pushes a
//! `ConnectWizard` modal onto the active agent's `active_modal` stack so
//! the existing modal-dispatch path picks it up. The actual submission
//! path lives in `app/modals.rs` (the `ActiveModal::ConnectWizard` arm)
//! so the dispatcher can close the modal before emitting the
//! `Action::ConnectCustomModel`.

use crate::app::actions::{Action, Effect};
use crate::app::app_view::{ActiveView, AppView, AuthState, ConnectWizardRequest};
use crate::views::connect_wizard::ConnectWizardState;
use crate::views::modal::ActiveModal;

/// Replay a parked `/connect` request once an agent view exists.
pub(super) fn dispatch_pending_connect_wizard(
    app: &mut AppView,
    request: ConnectWizardRequest,
) -> Vec<Effect> {
    match request {
        ConnectWizardRequest::Generic => dispatch_open_connect_wizard(app, None),
        ConnectWizardRequest::Preset(preset) => dispatch_open_connect_wizard(app, Some(&preset)),
    }
}

/// Push the `/connect` wizard modal onto the active agent. When no agent view
/// exists yet (welcome screen, dashboard) the request is parked on
/// `pending_connect_wizard` and replayed by the next dispatch that runs with an
/// agent on screen, so the command never silently no-ops.
pub(super) fn dispatch_open_connect_wizard(app: &mut AppView, preset: Option<&str>) -> Vec<Effect> {
    if preset.is_some_and(|preset| preset.eq_ignore_ascii_case("topodrive_cloud")) {
        let action = if !matches!(app.auth_state, AuthState::Done) || app.is_api_key_auth {
            Action::Login
        } else {
            Action::OpenSettingsFocus {
                key: "default_model",
            }
        };
        return super::router::dispatch(action, app);
    }
    if !matches!(app.active_view, ActiveView::Agent(_)) {
        // The wizard lives on the agent's modal stack, so a `/connect` typed on
        // the welcome screen (or the dashboard) had nothing to attach to and
        // silently did nothing. Park the request, start/ reveal a session, and
        // let the next dispatch with an agent on screen replay it.
        app.pending_connect_wizard = Some(match preset {
            Some(preset) => ConnectWizardRequest::Preset(preset.to_owned()),
            None => ConnectWizardRequest::Generic,
        });
        return super::session::lifecycle::leave_welcome_for_session(app);
    }
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    // Replace any existing connect wizard. A preset-only `/connect` command
    // opens the same form with that provider preselected.
    let mut state = ConnectWizardState::default();
    let preset = preset.unwrap_or("topodrive_cloud");
    if let Some(idx) = crate::views::connect_wizard::PRESET_DEFS
        .iter()
        .position(|definition| definition.id.eq_ignore_ascii_case(preset))
    {
        state.set_preset(idx);
    }
    agent.active_modal = Some(ActiveModal::ConnectWizard {
        state: Box::new(state),
    });
    vec![]
}
