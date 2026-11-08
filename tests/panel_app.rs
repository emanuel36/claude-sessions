//! `cst new --pick`, the Alt+n panel, running in a pane of a private tmux server.

mod support;

use std::fs;
use std::time::Duration;

use support::{write_history, TmuxApp};

/// How long the panel gets to start and list the folders.
const START: Duration = Duration::from_secs(5);

/// The panel in window `pick`, over a history of two folders: `alpha-proj` used a minute ago,
/// then `beta-proj`.
fn start() -> Option<TmuxApp> {
    let (tmux, env) = TmuxApp::server()?;
    let (alpha, beta) = (env.path().join("alpha-proj"), env.path().join("beta-proj"));
    for dir in [&alpha, &beta] {
        fs::create_dir(dir).expect("create dir");
    }
    write_history(
        &env,
        &[
            ("s1", &alpha, 60.0, "fix the api"),
            ("s2", &beta, 120.0, "write the docs"),
        ],
    );
    Some(TmuxApp::run(tmux, env, "pick", "new --pick"))
}

#[test]
fn panel_escape_exits() {
    let Some(panel) = start() else { return };
    panel.assert_shows("alpha-proj", START);
    panel.send("Escape");
    if !panel.within(Duration::from_secs(2), || {
        panel.format("#{pane_dead}") == "1"
    }) {
        panel.fail("the panel still runs after Esc");
    }
    assert_eq!(panel.format("#{pane_dead_status}"), "0");
}

#[test]
fn panel_typing_filters() {
    let Some(panel) = start() else { return };
    panel.assert_shows("alpha-proj", START);
    panel.assert_shows("beta-proj", START);
    panel.type_text("bet");
    panel.assert_shows("❯ bet", Duration::from_secs(2));
    if !panel.within(Duration::from_secs(2), || {
        !panel.screen().contains("alpha-proj")
    }) {
        panel.fail("alpha-proj still listed under the search \"bet\"");
    }
    assert!(panel.screen().contains("beta-proj"), "{}", panel.screen());
}
