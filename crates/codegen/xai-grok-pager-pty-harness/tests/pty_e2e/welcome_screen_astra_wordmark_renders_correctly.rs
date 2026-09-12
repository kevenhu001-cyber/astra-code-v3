// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// **Welcome screen renders the pixel-block ASTRA wordmark correctly.**
///
/// The central Braille hero logo was removed by design (the stacked welcome
/// layout allocates zero logo rows — see `views::welcome::mod`). The sole
/// brand mark is now the top pixel-block ASTRA wordmark painted with Unicode
/// full-block characters (`█`, U+2588).
///
/// This test keeps the original regression intent of the retired
/// `welcome_screen_braille_logo_renders_correctly` case: a broken writer
/// thread (raw bytes through a code-page-dependent API instead of UTF-8) or a
/// missing console code-page setup mangles multi-byte characters into
/// single-byte garbage. `█` is a 3-byte UTF-8 sequence that appears only in
/// the wordmark on the welcome screen (menu labels are pure ASCII), so its
/// intact presence proves multi-byte output survives the PTY round-trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn welcome_screen_astra_wordmark_renders_correctly() {
    let content = ContentController::start().await.expect("start content");

    let binary = pager_binary().expect("resolve pager binary");
    // Use a tall terminal so the wordmark slot has room (the paint no-ops
    // when the area is narrower/shorter than the wordmark).
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .expect("spawn pager");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    let screen = harness.screen_contents();

    assert!(
        screen.contains('\u{2588}'),
        "Block character █ (U+2588) not found in screen — the ASTRA wordmark \
         may be garbled or skipped.\n\
         Screen contents:\n{screen}"
    );

    harness.quit().expect("clean quit");
}
