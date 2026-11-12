//! The commands tmux runs on window changes: `cst layout`, `cst step` and `cst focus`.
//!
//! The sidebar and the landing page learn about the screen from keys tmux sends them, which
//! Claude Code never sees: [`SHOWN`] when their window comes on screen, [`HIDDEN`] when it
//! leaves it, and [`LAYOUT`] when windows or panes change.

use crate::claude::hook;
use crate::core::state::Record;
use crate::term::tmux::{self, Cli, Pane, Runner};
use crate::ui::sidebar;

/// Percent of the width beside the sidebar that the session in charge keeps once teammates join
/// it; they share the rest, stacked.
const MAIN_PERCENT: u32 = 60;
/// Fewest panes beside the sidebar, the session's own included, that make a team to arrange.
const TEAM_PANES: usize = 3;
/// Narrowest window that is arranged: below it the columns would be unusable.
const MIN_WIDTH: u32 = 80;
/// What [`arrange_teams`] asks for: one row per pane, with its place in the window.
const TEAM_FORMAT: &str = "#{window_id}\t#{window_width}\t#{window_height}\t#{pane_id}\t\
    #{@cs-sidebar}\t#{@cs-landing}\t#{window_zoomed_flag}\t#{pane_left}\t#{pane_top}\t\
    #{pane_width}\t#{pane_height}";

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
/// a sidebar is left, arranges the windows with teammates, fits every sidebar to its width and
/// tells the panes on screen.
///
/// tmux runs it when a window opens, a pane splits, exits or is killed, a layout is selected, and
/// a window is resized, renamed or closed.
pub fn layout(window: Option<&str>, t: &dyn Runner) {
    if let Some(window) = window {
        tmux::add_sidebar(window, t);
    }
    for window in lone_sidebars(&tmux::panes(t)) {
        tmux::close_window(&window, t);
    }
    arrange_teams(t);
    tmux::fit_sidebars(t);
    for pane in t
        .run(&["list-panes", "-a", "-f", ON_SCREEN, "-F", "#{pane_id}"])
        .lines()
    {
        send(pane, LAYOUT, t);
    }
}

/// A pane's id and place: left, top, width and height in cells.
type Slot = (u32, [u32; 4]);

/// A window's panes, as [`TEAM_FORMAT`] lists them.
struct Team {
    window: String,
    width: u32,
    height: u32,
    sidebar: Option<Slot>,
    panes: Vec<Slot>,
}

/// The windows with teammates, from the rows of [`TEAM_FORMAT`]. A window with a landing page, a
/// zoomed pane or no sidebar is left out, and so is a row that does not parse.
fn teams(listing: &str) -> Vec<Team> {
    let mut teams: Vec<Team> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for line in listing.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let [window, width, height, pane, sidebar, landing, zoomed, left, top, w, h] = f[..] else {
            continue;
        };
        let nums: Option<Vec<u32>> = [width, height, left, top, w, h]
            .iter()
            .map(|n| n.parse().ok())
            .collect();
        let (Some(n), Some(id)) = (nums, pane.strip_prefix('%').and_then(|n| n.parse().ok()))
        else {
            continue;
        };
        if !landing.is_empty() || zoomed == "1" {
            skipped.push(window.to_owned());
        }
        if teams.last().is_none_or(|t| t.window != window) {
            teams.push(Team {
                window: window.to_owned(),
                width: n[0],
                height: n[1],
                sidebar: None,
                panes: Vec::new(),
            });
        }
        let team = teams.last_mut().expect("pushed above");
        let slot = (id, [n[2], n[3], n[4], n[5]]);
        if sidebar.is_empty() {
            team.panes.push(slot);
        } else {
            team.sidebar = Some(slot);
        }
    }
    teams.retain(|t| {
        t.sidebar.is_some() && t.panes.len() >= TEAM_PANES && !skipped.contains(&t.window)
    });
    teams
}

/// tmux's checksum of a layout string, which `select-layout` checks.
fn checksum(layout: &str) -> u16 {
    layout.bytes().fold(0u16, |sum, b| {
        (sum >> 1)
            .wrapping_add((sum & 1) << 15)
            .wrapping_add(u16::from(b))
    })
}

/// Where each pane goes: the sidebar at its width, the first pane in a column of
/// [`MAIN_PERCENT`] of the rest and the teammates stacked evenly in the other column.
fn team_slots(team: &Team) -> Option<(Slot, Slot, Vec<Slot>)> {
    let (sidebar, _) = team.sidebar?;
    let (first, mates) = team.panes.split_first()?;
    let side = tmux::SIDEBAR_WIDTH;
    let area = team.width.checked_sub(side + 1)?;
    let n = u32::try_from(mates.len()).ok()?;
    if team.width < MIN_WIDTH || team.height < 2 * n {
        return None;
    }
    let (h, main) = (team.height, area * MAIN_PERCENT / 100);
    let rest = area - main - 1;
    // The rows left after the borders between the teammates, the first ones taking the extra
    let rows = h - (n - 1);
    let mut y = 0;
    let stack = (0..n)
        .zip(mates)
        .map(|(i, (id, _))| {
            let height = rows / n + u32::from(i < rows % n);
            let slot = (*id, [side + main + 2, y, rest, height]);
            y += height + 1;
            slot
        })
        .collect();
    Some((
        (sidebar, [0, 0, side, h]),
        (first.0, [side + 1, 0, main, h]),
        stack,
    ))
}

/// The layout string that places the panes as [`team_slots`] says.
fn layout_string(width: u32, height: u32, sidebar: Slot, main: Slot, stack: &[Slot]) -> String {
    let leaf = |(id, [x, y, w, h]): &Slot| format!("{w}x{h},{x},{y},{id}");
    let right = match stack {
        [only] => leaf(only),
        _ => {
            let x = stack[0].1[0];
            let w = stack[0].1[2];
            let cells: Vec<String> = stack.iter().map(leaf).collect();
            format!("{w}x{height},{x},0[{}]", cells.join(","))
        }
    };
    let body = format!(
        "{width}x{height},0,0{{{},{},{right}}}",
        leaf(&sidebar),
        leaf(&main)
    );
    format!("{:04x},{body}", checksum(&body))
}

/// The window option that keeps the places the panes took after the last arrangement.
const ARRANGED: &str = "@cs-arranged";
/// What [`ARRANGED`] holds while a layout is being selected, so the run the hook starts for it
/// does not select it again.
const BUSY: &str = "busy";

/// Panes and their places, as one line to compare.
fn places(mut slots: Vec<Slot>) -> String {
    slots.sort_unstable();
    let slot = |(id, [l, t, w, h]): &Slot| format!("{id}:{l},{t},{w},{h}");
    slots.iter().map(slot).collect::<Vec<_>>().join(";")
}

/// All the panes of a team, the sidebar's included.
fn slots(team: &Team) -> Vec<Slot> {
    team.sidebar.into_iter().chain(team.panes.clone()).collect()
}

/// Gives a window where teammates joined the session a layout that keeps the session in charge
/// readable: it at the left with most of the width, the teammates stacked at the right. Claude
/// Code applies `main-vertical` as they join, which leaves them little room each.
///
/// Selecting a layout runs the hook that ends up here again, so a window is left alone when its
/// panes are where the layout wanted them or where the last arrangement left them: tmux may
/// place them elsewhere (a pane border status takes a row, panes in another order take other
/// cells), and trying again would only repeat that. Claude Code's own `main-vertical` moves
/// them, which arranges the window again. The places are compared rather than tmux's layout
/// string, which tmux 3.7 writes in another format than `select-layout` takes.
fn arrange_teams(t: &dyn Runner) {
    for team in teams(&t.run(&["list-panes", "-a", "-F", TEAM_FORMAT])) {
        let Some((sidebar, main, stack)) = team_slots(&team) else {
            continue;
        };
        let now = places(slots(&team));
        let wanted = places(
            [sidebar, main]
                .into_iter()
                .chain(stack.iter().copied())
                .collect(),
        );
        let last = t.run(&["show-option", "-wqv", "-t", &team.window, ARRANGED]);
        if now == wanted || now == last.trim() || last.trim() == BUSY {
            continue;
        }
        let layout = layout_string(team.width, team.height, sidebar, main, &stack);
        t.run(&["set-option", "-w", "-t", &team.window, ARRANGED, BUSY]);
        t.run(&["select-layout", "-t", &team.window, &layout]);
        let after = t.run(&["list-panes", "-t", &team.window, "-F", TEAM_FORMAT]);
        let place = teams(&after)
            .first()
            .map(|arranged| places(slots(arranged)));
        match place {
            Some(place) => t.run(&["set-option", "-w", "-t", &team.window, ARRANGED, &place]),
            None => t.run(&["set-option", "-wu", "-t", &team.window, ARRANGED]),
        };
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
    use crate::term::layout::{
        layout, show, step, team_slots, teams, HIDDEN, LAST_WINDOW, LAYOUT, SHOWN,
    };
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

    /// The rows of one window 185 wide and 47 high, each pane as `(id, sidebar, [left, top,
    /// width, height])`.
    fn team_rows(landing: &str, zoomed: &str, panes: &[(&str, &str, [u32; 4])]) -> String {
        panes
            .iter()
            .map(|(id, sidebar, [l, t, w, h])| {
                let head = format!("@7\t185\t47\t{id}\t{sidebar}\t{landing}\t{zoomed}");
                format!("{head}\t{l}\t{t}\t{w}\t{h}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A team as Claude Code leaves it with `main-vertical`: the sidebar stretched, the rest
    /// stacked.
    const STACKED: [(&str, &str, [u32; 4]); 4] = [
        ("%1", "1", [0, 0, 80, 47]),
        ("%2", "", [81, 0, 104, 15]),
        ("%3", "", [81, 16, 104, 15]),
        ("%4", "", [81, 32, 104, 15]),
    ];

    /// [`STACKED`] arranged.
    const PLACED: [(&str, &str, [u32; 4]); 4] = [
        ("%1", "1", [0, 0, 29, 47]),
        ("%2", "", [30, 0, 93, 47]),
        ("%3", "", [124, 0, 61, 23]),
        ("%4", "", [124, 24, 61, 23]),
    ];

    fn team_server(rows: String) -> FakeTmux {
        FakeTmux::new(move |args| match args {
            ["list-panes", "-a", "-F", f, ..] if f.starts_with("#{window_id}\t") => rows.clone(),
            _ => String::new(),
        })
    }

    #[test]
    fn a_team_keeps_the_main_pane_big_and_stacks_the_rest() {
        let found = teams(&team_rows("", "0", &STACKED));
        let (sidebar, main, stack) = team_slots(&found[0]).expect("a layout");
        assert_eq!(sidebar, (1, PLACED[0].2));
        assert_eq!(main, (2, PLACED[1].2));
        assert_eq!(stack, [(3, PLACED[2].2), (4, PLACED[3].2)]);
    }

    #[test]
    fn layout_arranges_a_team_once() {
        let t = team_server(team_rows("", "0", &STACKED));
        layout(None, &t);
        let select = t.calls_to("select-layout");
        assert_eq!(select.len(), 1);
        assert_eq!(select[0][..3], ["select-layout", "-t", "@7"]);
        let t = team_server(team_rows("", "0", &PLACED));
        layout(None, &t);
        assert!(t.calls_to("select-layout").is_empty());
    }

    #[test]
    fn a_window_stays_as_the_last_arrangement_left_it() {
        // tmux placed the panes elsewhere than asked; asking again would only repeat that
        let rows = team_rows("", "0", &STACKED);
        let kept = "1:0,0,80,47;2:81,0,104,15;3:81,16,104,15;4:81,32,104,15";
        let t = FakeTmux::new(move |args| match args {
            ["list-panes", "-a", "-F", ..] => rows.clone(),
            ["show-option", ..] => kept.into(),
            _ => String::new(),
        });
        layout(None, &t);
        assert!(t.calls_to("select-layout").is_empty());
    }

    #[test]
    fn a_window_that_is_not_a_team_is_left_alone() {
        // Only two panes beside the sidebar, a zoomed pane and a landing page
        assert!(teams(&team_rows("", "0", &STACKED[..3])).is_empty());
        assert!(teams(&team_rows("", "1", &STACKED)).is_empty());
        assert!(teams(&team_rows("1", "0", &STACKED)).is_empty());
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
