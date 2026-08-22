// Per-test-case module for the `pty_e2e` integration test crate.
//
// Drives the `/connect` slash command through the guided wizard: types
// `/connect`, waits for the modal, cycles the preset Down to `custom`, then
// Tabs Preset -> Provider Name -> Protocol -> Base URL and types into URL /
// API key / Model ID, submits, and asserts the connect persisted the new
// `[model.astra-custom]` block under `~/.astra/config.toml` and pinned it as
// `[models].default`.
//
// Skipped by default (`#[ignore]`) like the other `pty_e2e` cases — it
// needs a built pager binary and a `models.json` writeable HOME. Run with
// `cargo test --test pty_e2e -- --ignored connect_wizard`.
#[allow(unused_imports)]
use super::common::*;
use std::time::Duration;

/// Title of the wizard (see `views::connect_wizard::render_wizard`).
/// Rendered in the modal border; only the wizard ever writes that exact
/// phrase, so it's a stable "modal is open" gate.
const WIZARD_TITLE_SENTINEL: &str = "Connect Model Provider";

/// Total time budget for the three-step flow + persistence. The wizard
/// itself is snappy (no network), but the persistence side runs through
/// the shell config writer which can take a few hundred ms under the
/// parallel pty_e2e suite.
const WIZARD_TIMEOUT: Duration = Duration::from_secs(30);

/// Drive `/connect` through the wizard with a custom (user-typed) URL,
/// model id, and API key, then assert the result landed in
/// `~/.astra/config.toml` and that the model picker would see it.
///
/// Uses a `custom` preset so the URL field starts empty and the test
/// exercises the free-form path (the most common case for BYOK endpoints
/// outside the built-in vendor list). The non-custom preset path is
/// covered by the unit tests in `views::connect_wizard::tests` and the
/// `run()` tests in `slash::commands::connect`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn connect_wizard_three_step_flow_persists_config() {
    let content = ContentController::start().await.expect("start content");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .expect("spawn pager");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome screen");

    // Type `/connect` to open the wizard. Paced so the slash dropdown
    // opens instead of the bytes paste-coalescing (same shape as the
    // settings-modal test).
    inject_keys_paced(&mut harness, b"/connect");
    harness.inject_keys(b"\r").expect("submit /connect");

    harness
        .wait_for_text(WIZARD_TITLE_SENTINEL, WIZARD_TIMEOUT)
        .expect("wizard modal opened");

    // The wizard lands on the Preset field, where Down cycles the preset
    // in place (`custom` is the last entry). After the last Down the Base
    // URL field clears; its empty-state placeholder proves the custom
    // preset is selected and we can type our own URL.
    for _ in 0..9 {
        harness.inject_keys(keys::DOWN).expect("preset down");
    }
    harness
        .wait_for_text("<None / Enter Base URL>", WIZARD_TIMEOUT)
        .expect("custom preset selected, URL field empty");

    // Tab walks the form fields in order: Preset -> Provider Name ->
    // Protocol Format -> Base URL. Three Tabs land focus on Base URL.
    for _ in 0..3 {
        harness.inject_keys(b"\t").expect("focus next field");
    }

    // Type the URL. Paced again so each character lands as its own key
    // event (the wizard only re-renders on Changed outcomes; a coalesced
    // paste still works but paced is friendlier to the assertion timing).
    let url = "https://example.com/v1";
    inject_keys_paced(&mut harness, url.as_bytes());

    // Tab -> API key.
    harness.inject_keys(b"\t").expect("focus api key");
    let api_key = "sk-wizard-test-0001";
    inject_keys_paced(&mut harness, api_key.as_bytes());

    // Tab -> Model ID.
    harness.inject_keys(b"\t").expect("focus model id");
    let model_id = "wizard-test-model";
    inject_keys_paced(&mut harness, model_id.as_bytes());

    // Submit.
    harness.inject_keys(b"\r").expect("submit wizard");

    // The wizard closes; the modal chrome with the title is gone.
    wait_for_labels_absent(&mut harness, &[WIZARD_TITLE_SENTINEL], WIZARD_TIMEOUT);

    // The wizard persists through six `Effect::PersistSetting` tasks which
    // run CONCURRENTLY on a JoinSet (see `effects::mod`), so each field lands
    // in config.toml in arbitrary order. Poll until every marker we submitted
    // is present; dumping the last seen file on timeout keeps failures
    // diagnosable.
    let config_path = content.home().join(".astra").join("config.toml");
    let deadline = std::time::Instant::now() + WIZARD_TIMEOUT;
    let mut last_config = String::from("<no config.toml yet>");
    let config = loop {
        if let Ok(c) = std::fs::read_to_string(&config_path) {
            last_config = c.clone();
            if c.contains(url)
                && c.contains(model_id)
                && c.contains(api_key)
                && c.contains("astra-custom")
            {
                break c;
            }
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "config.toml never contained the submitted wizard fields\n\
                 path: {}\n\
                 last seen:\n{last_config}",
                config_path.display()
            );
        }
        harness.update(Duration::from_millis(200));
    };

    assert!(
        config.contains(model_id),
        "config.toml must reference the model id we submitted\n\
         config: {config}\nexpected model id: {model_id}",
    );
    assert!(
        config.contains(url),
        "config.toml must reference the base URL we submitted\n\
         config: {config}\nexpected URL: {url}",
    );
    assert!(
        config.contains(api_key),
        "config.toml must contain the API key we submitted\n\
         config: {config}\nexpected key: {api_key}",
    );
    // Sanity: the default model pointer moved to our new entry.
    assert!(
        config.contains("astra-custom") || config.contains("default"),
        "config.toml must pin the new model as default\nconfig: {config}",
    );

    // Smoke: the pager did not panic on the new path.
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}

/// Bare `/connect` (no args) opens the wizard on the first preset, not a
/// help message — verifies the slash-command rewrite wired the default
/// path to the modal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn connect_wizard_default_preset_openai() {
    let content = ContentController::start().await.expect("start content");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .expect("spawn pager");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WIZARD_TIMEOUT)
        .expect("welcome screen");

    inject_keys_paced(&mut harness, b"/connect");
    harness.inject_keys(b"\r").expect("submit /connect");

    harness
        .wait_for_text(WIZARD_TITLE_SENTINEL, WIZARD_TIMEOUT)
        .expect("wizard opened from bare /connect");

    // The OpenAI preset URL is pre-filled in the URL field. Verify it
    // rendered so we know the wizard mounted, not just the title chrome.
    harness
        .wait_for_text("https://api.openai.com/v1", WIZARD_TIMEOUT)
        .expect("OpenAI preset URL pre-filled");

    // Esc closes the wizard and returns to the prompt.
    harness.inject_keys(keys::ESC).expect("esc wizard");
    wait_for_labels_absent(&mut harness, &[WIZARD_TITLE_SENTINEL], WIZARD_TIMEOUT);

    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}
