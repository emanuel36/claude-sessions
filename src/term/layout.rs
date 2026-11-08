//! The commands tmux runs on window changes: `cst layout`, `cst step` and `cst focus`.
//!
//! The sidebar and the landing page learn about the screen from keys tmux sends them, which
//! Claude Code never sees: [`SHOWN`] when their window comes on screen, [`HIDDEN`] when it
//! leaves it, and [`LAYOUT`] when windows or panes change.

use crate::claude::hook;
use crate::core::state::Record;
use crate::term::tmux::{self, Cli, Pane, Runner};
use crate::ui::sidebar;

/// Tells a sidebar or landing page its window came on screen.
pub const SHOWN: &str = "F12";
/// Tells a sidebar or landing page its window left the screen.
pub const HIDDEN: &str = "F11";
/// Tells the sidebar or landing page on screen that windows or panes changed.
pub const LAYOUT: &str = "F10";
/// The server option that holds the window shown last, to send it [`HIDDEN`] once another one
/// is shown.
pub const LAST_WINDOW: &str = "@cs-last-window";

/// The sidebar and landing panes of the windows on screen.
const ON_SCREEN: &str = "#{&&:#{window_active},#{||:#{@cs-sidebar},#{@cs-landing}}}";

/// `cst layout [WINDOW]`: puts the sidebar in `window` if given, closes the windows where only
/// a sidebar is left, fits every sidebar to its width and tells the panes on screen.
///
/// tmux runs it when a window opens, a pane splits, exits or is killed, and a window is resized,
/// renamed or closed.
pub fn layout(window: Option<&str>, t: &dyn Runner) {
    if let Some(window) = window {
        tmux::add_sidebar(window, t);
    }
    for window in lone_sidebars(&tmux::panes(t)) {
        tmux::close_window(&window, t);
    }
    tmux::fit_sidebars(t);
    for pane in t
        .run(&["list-panes", "-a", "-f", ON_SCREEN, "-F", "#{pane_id}"])
        .lines()
    {
        send(pane, LAYOUT, t);
    }
}

/// The windows whose panes are all sidebars: the session in them has exited.
fn lone_sidebars(panes: &[Pane]) -> Vec<String> {
    tmux::window_entries(panes)
        .into_iter()
        .map(|(id, _, _)| id)
        .filter(|id| {
            panes
                .iter()
                .filter(|p| &p.window_id == id)
                .all(|p| p.sidebar)
        })
        .collect()
}

/// `cst step up|down WINDOW`: goes to the session `offset` places from `window`, in the order the
/// sidebar lists them.
///
/// # Arguments
/// * `window`: The window on screen.
/// * `offset`: 1 for the session below, -1 for the one above.
/// * `records`: The live sessions, most urgent first.
/// * `socket`: The server's socket path; sessions of other servers are left out.
/// * `t`: Runs tmux on that server.
pub fn step(window: &str, offset: i32, records: &[Record], socket: &str, t: &dyn Runner) {
    let entries = sidebar::load_entries(records, &tmux::panes(t), socket);
    if let Some(target) = sidebar::step(&entries, window, offset) {
        t.run(&["select-window", "-t", &target]);
    }
}

/// Tells the sidebar or landing page of the window just shown, the one `pane` is in, that it is
/// on screen, and the one of the window shown before that it is not.
pub fn show(pane: &str, t: &dyn Runner) {
    let panes = tmux::panes(t);
    let Some(current) = panes.iter().find(|p| p.pane_id == pane) else {
        return;
    };
    let current = current.window_id.as_str();
    let last = t.run(&["show-option", "-sqv", LAST_WINDOW]);
    if last != current {
        if let Some(hidden) = signalled(&panes, &last) {
            send(hidden, HIDDEN, t);
        }
    }
    if let Some(shown) = signalled(&panes, current) {
        send(shown, SHOWN, t);
    }
    if last != current {
        t.run(&["set-option", "-s", LAST_WINDOW, current]);
    }
}

/// `cst focus --pane P --socket S`: a window came on screen. Tells the sidebars and landing
/// page as [`show`] does, then marks the finished session in `pane` seen.
///
/// Never fails: tmux shows a failed command's output over the pane, so errors go to the log.
pub fn focus(pane: &str, socket: &str, now: f64) {
    show(pane, &Cli::at(socket));
    hook::seen(pane, socket, now);
}

/// The pane of `window` that takes the keys: its sidebar or landing page.
fn signalled<'a>(panes: &'a [Pane], window: &str) -> Option<&'a str> {
    panes
        .iter()
        .find(|p| p.window_id == window && (p.sidebar || p.landing))
        .map(|p| p.pane_id.as_str())
}

fn send(pane: &str, key: &str, t: &dyn Runner) {
    t.run(&["send-keys", "-t", pane, key]);
}

#[cfg(test)]
mod tests {
    use crate::core::state::{new_record, Mux, Record, Status};
    use crate::term::layout::{layout, show, step, HIDDEN, LAST_WINDOW, LAYOUT, SHOWN};
    use crate::term::tmux::fake::FakeTmux;

    fn listing(rows: &[[&str; 7]]) -> String {
        rows.iter()
            .map(|r| r.join("\t"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The landing page @1, sessions @2 and @3 with a sidebar each, and @4 where only the
    /// sidebar is left.
    fn server() -> String {
        listing(&[
            ["%1", "@1", "1", "1", "", "1", "sessions"],
            ["%2", "@2", "1", "2", "1", "", "api"],
            ["%3", "@2", "2", "2", "", "", "api"],
            ["%4", "@3", "1", "3", "1", "", "web"],
            ["%5", "@3", "2", "3", "", "", "web"],
            ["%6", "@4", "1", "4", "1", "", "gone"],
        ])
    }

    /// Answers as [`server`], with `last` as the window shown last and `active` as the panes
    /// that show their window's sidebar or landing page on screen.
    fn reply(last: &'static str, active: &'static str) -> FakeTmux {
        FakeTmux::new(move |args| match args {
            ["list-panes", "-a", "-F", ..] => server(),
            ["list-panes", "-a", "-f", ..] => active.into(),
            ["show-option", ..] => last.into(),
            _ => String::new(),
        })
    }

    fn sent(t: &FakeTmux) -> Vec<(String, String)> {
        t.calls_to("send-keys")
            .into_iter()
            .map(|c| (c[2].clone(), c[3].clone()))
            .collect()
    }

    fn pair(pane: &str, key: &str) -> (String, String) {
        (pane.to_owned(), key.to_owned())
    }

    #[test]
    fn layout_adds_a_sidebar_to_a_window_or_fits_them_all() {
        let t = reply("", "");
        layout(Some("@5"), &t);
        let calls = t.calls();
        assert_eq!(calls[0], ["list-panes", "-t", "@5", "-F", "#{@cs-sidebar}"]);
        assert_eq!(calls[1][0], "split-window");
        assert_eq!(calls[1][6], "@5");
        let fits = t.calls_to("resize-pane");
        assert_eq!(fits.len(), 3);
        let t = reply("", "");
        layout(None, &t);
        assert!(t.calls_to("split-window").is_empty());
        assert_eq!(t.calls_to("resize-pane").len(), 3);
    }

    #[test]
    fn layout_closes_a_window_where_only_the_sidebar_is_left() {
        let t = reply("", "");
        layout(None, &t);
        assert_eq!(t.calls_to("kill-window"), [["kill-window", "-t", "@4"]]);
    }

    #[test]
    fn layout_tells_the_panes_on_screen_that_the_layout_changed() {
        let t = reply("", "%2\n%1");
        layout(None, &t);
        let ask = &t
            .calls_to("list-panes")
            .into_iter()
            .find(|c| c[2] == "-f")
            .expect("asks for the panes on screen");
        assert_eq!(
            ask[3],
            "#{&&:#{window_active},#{||:#{@cs-sidebar},#{@cs-landing}}}"
        );
        assert_eq!(sent(&t), [pair("%2", LAYOUT), pair("%1", LAYOUT)]);
    }

    fn at(id: &str, status: Status, pane: &str) -> Record {
        Record {
            status,
            mux: Some(Mux {
                socket: "/s".into(),
                pane: pane.into(),
            }),
            ..new_record(id)
        }
    }

    #[test]
    fn step_goes_to_the_next_session_as_the_sidebar_lists_them() {
        // @3 needs the user, so it is listed before @2
        let records = [
            at("b", Status::Permission, "%5"),
            at("a", Status::Done, "%3"),
        ];
        let t = reply("", "");
        step("@3", 1, &records, "/s", &t);
        assert_eq!(t.calls_to("select-window"), [["select-window", "-t", "@2"]]);
        let t = reply("", "");
        step("@3", -1, &records, "/s", &t);
        assert_eq!(t.calls_to("select-window"), [["select-window", "-t", "@2"]]);
    }

    #[test]
    fn step_without_sessions_selects_nothing() {
        let t = reply("", "");
        step("@3", 1, &[], "/s", &t);
        assert!(t.calls_to("select-window").is_empty());
    }

    #[test]
    fn show_hides_the_window_shown_before_and_shows_this_one() {
        let t = reply("@3", "");
        show("%3", &t);
        assert_eq!(sent(&t), [pair("%4", HIDDEN), pair("%2", SHOWN)]);
        assert_eq!(
            t.calls_to("show-option"),
            [["show-option", "-sqv", LAST_WINDOW]]
        );
        assert_eq!(
            t.calls_to("set-option"),
            [["set-option", "-s", LAST_WINDOW, "@2"]]
        );
    }

    #[test]
    fn show_on_the_window_already_shown_only_shows_it() {
        let t = reply("@2", "");
        show("%2", &t);
        assert_eq!(sent(&t), [pair("%2", SHOWN)]);
        assert!(t.calls_to("set-option").is_empty());
    }

    #[test]
    fn show_wakes_the_landing_page_and_skips_a_window_with_neither() {
        let t = reply("", "");
        show("%1", &t);
        assert_eq!(sent(&t), [pair("%1", SHOWN)]);
        // A window shown before that has no sidebar any more gets nothing
        let t = FakeTmux::new(|args| match args {
            ["list-panes", ..] => listing(&[
                ["%1", "@1", "1", "1", "", "1", "sessions"],
                ["%7", "@5", "1", "5", "", "", "bare"],
            ]),
            ["show-option", ..] => "@5".into(),
            _ => String::new(),
        });
        show("%1", &t);
        assert_eq!(sent(&t), [pair("%1", SHOWN)]);
        show("%7", &t);
        assert_eq!(sent(&t), [pair("%1", SHOWN)]);
    }

    #[test]
    fn show_for_a_pane_that_is_gone_does_nothing() {
        let t = reply("@3", "");
        show("%99", &t);
        assert!(sent(&t).is_empty());
        assert!(t.calls_to("set-option").is_empty());
    }
}
