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
/// What `cst layout` lists of each pane ahead of [`tmux::PANES_FORMAT`]: whether its window is
/// its session's active one, the window's size and zoom, and the pane's place in it.
const PLACE_FORMAT: &str = "#{window_active}\t#{window_width}\t#{window_height}\t\
    #{window_zoomed_flag}\t#{pane_left}\t#{pane_top}\t#{pane_width}\t#{pane_height}";

/// Tells a sidebar or landing page its window came on screen.
pub const SHOWN: &str = "F12";
/// Tells a sidebar or landing page its window left the screen.
pub const HIDDEN: &str = "F11";
/// Tells the sidebar or landing page on screen that windows or panes changed.
pub const LAYOUT: &str = "F10";
/// The server option that holds the window shown last, to send it [`HIDDEN`] once another one
/// is shown.
pub const LAST_WINDOW: &str = "@cs-last-window";

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
    let mut listing = list(&["-a"], t);
    let lone = lone_sidebars(&listing.panes);
    for window in &lone {
        tmux::close_window(window, t);
    }
    // Closing a window takes its panes away and may bring another window on screen
    if !lone.is_empty() {
        listing = list(&["-a"], t);
    }
    arrange_teams(&listing, t);
    tmux::fit_sidebars_in(&listing.panes, t);
    for (pane, place) in listing.panes.iter().zip(&listing.places) {
        if place.as_ref().is_some_and(|p| p.active) && (pane.sidebar || pane.landing) {
            send(&pane.pane_id, LAYOUT, t);
        }
    }
}

/// A pane's place, as [`PLACE_FORMAT`] lists it.
struct Place {
    /// Whether the pane's window is its session's active one.
    active: bool,
    /// Whether a pane of the window is zoomed.
    zoomed: bool,
    /// The window's width and height in cells.
    window: [u32; 2],
    /// The pane's left, top, width and height in cells.
    cells: [u32; 4],
}

/// Panes and, at the same index, their places.
#[derive(Default)]
struct Listing {
    panes: Vec<Pane>,
    places: Vec<Option<Place>>,
}

/// Lists the panes `scope` names (`-a`, or `-t WINDOW`) with their places.
fn list(scope: &[&str], t: &dyn Runner) -> Listing {
    let format = format!("{PLACE_FORMAT}\t{}", tmux::PANES_FORMAT);
    let mut args = vec!["list-panes"];
    args.extend_from_slice(scope);
    args.extend(["-F", &format]);
    parse(&t.run(&args))
}

/// The rows of [`list`]; a row whose pane does not parse is left out, and one whose place
/// does not keeps the pane with no place, so a window is never closed over a bad number.
fn parse(out: &str) -> Listing {
    let mut listing = Listing::default();
    for line in out.lines() {
        let f: Vec<&str> = line.splitn(9, '\t').collect();
        let [active, width, height, zoomed, left, top, w, h, rest] = f[..] else {
            continue;
        };
        let Some(pane) = tmux::parse_pane(rest) else {
            continue;
        };
        let nums = [width, height, left, top, w, h].map(|n| n.parse::<u32>().ok());
        let place = match nums {
            [Some(width), Some(height), Some(left), Some(top), Some(w), Some(h)] => Some(Place {
                active: active == "1",
                zoomed: zoomed == "1",
                window: [width, height],
                cells: [left, top, w, h],
            }),
            _ => None,
        };
        listing.panes.push(pane);
        listing.places.push(place);
    }
    listing
}

/// A pane's id and place: left, top, width and height in cells.
type Slot = (u32, [u32; 4]);

/// A window's panes, split into its sidebar and the rest.
struct Team<'a> {
    window: &'a str,
    width: u32,
    height: u32,
    /// The window has a landing page or a zoomed pane, so it is not arranged.
    skip: bool,
    sidebar: Option<Slot>,
    panes: Vec<Slot>,
}

/// The windows with teammates in `listing`. A window with a landing page, a zoomed pane or no
/// sidebar is left out.
fn teams(listing: &Listing) -> Vec<Team<'_>> {
    let mut teams: Vec<Team> = Vec::new();
    for (pane, place) in listing.panes.iter().zip(&listing.places) {
        let (Some(place), Some(id)) = (
            place,
            pane.pane_id.strip_prefix('%').and_then(|n| n.parse().ok()),
        ) else {
            continue;
        };
        if teams.last().is_none_or(|t| t.window != pane.window_id) {
            teams.push(Team {
                window: &pane.window_id,
                width: place.window[0],
                height: place.window[1],
                skip: false,
                sidebar: None,
                panes: Vec::new(),
            });
        }
        let team = teams.last_mut().expect("pushed above");
        team.skip |= pane.landing || place.zoomed;
        let slot = (id, place.cells);
        if pane.sidebar {
            team.sidebar = Some(slot);
        } else {
            team.panes.push(slot);
        }
    }
    teams.retain(|t| !t.skip && t.sidebar.is_some() && t.panes.len() >= TEAM_PANES);
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
fn arrange_teams(listing: &Listing, t: &dyn Runner) {
    for team in teams(listing) {
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
        let last = t.run(&["show-option", "-wqv", "-t", team.window, ARRANGED]);
        if now == wanted || now == last.trim() || last.trim() == BUSY {
            continue;
        }
        let layout = layout_string(team.width, team.height, sidebar, main, &stack);
        t.run(&["set-option", "-w", "-t", team.window, ARRANGED, BUSY]);
        t.run(&["select-layout", "-t", team.window, &layout]);
        let after = list(&["-t", team.window], t);
        let place = teams(&after)
            .first()
            .map(|arranged| places(slots(arranged)));
        match place {
            Some(place) => t.run(&["set-option", "-w", "-t", team.window, ARRANGED, &place]),
            None => t.run(&["set-option", "-wu", "-t", team.window, ARRANGED]),
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
fn show(pane: &str, t: &dyn Runner) {
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
/// page as `show` does, then marks the finished session in `pane` seen.
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
        layout, parse, show, step, team_slots, teams, HIDDEN, LAST_WINDOW, LAYOUT, PLACE_FORMAT,
        SHOWN,
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
    const SERVER: [[&str; 7]; 6] = [
        ["%1", "@1", "1", "1", "", "1", "sessions"],
        ["%2", "@2", "1", "2", "1", "", "api"],
        ["%3", "@2", "2", "2", "", "", "api"],
        ["%4", "@3", "1", "3", "1", "", "web"],
        ["%5", "@3", "2", "3", "", "", "web"],
        ["%6", "@4", "1", "4", "1", "", "gone"],
    ];

    /// [`SERVER`] as `cst layout` lists it, with the windows in `active` on screen.
    fn placed(active: &[&str]) -> String {
        SERVER
            .iter()
            .map(|r| {
                let on = u8::from(active.contains(&r[1]));
                format!("{on}\t100\t50\t0\t0\t0\t10\t10\t{}", r.join("\t"))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Answers as [`SERVER`], with `last` as the window shown last and `active` as the windows
    /// on screen.
    fn reply(last: &'static str, active: &'static [&'static str]) -> FakeTmux {
        FakeTmux::new(move |args| match args {
            ["list-panes", "-a", "-F", f] if f.starts_with(PLACE_FORMAT) => placed(active),
            ["list-panes", "-a", "-F", ..] => listing(&SERVER),
            ["show-option", ..] => last.into(),
            _ => String::new(),
        })
    }

    /// How many times the whole server's panes were listed.
    fn listings(t: &FakeTmux) -> usize {
        t.calls_to("list-panes")
            .iter()
            .filter(|c| c[1] == "-a")
            .count()
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
        let t = reply("", &[]);
        layout(Some("@5"), &t);
        let calls = t.calls();
        assert_eq!(calls[0], ["list-panes", "-t", "@5", "-F", "#{@cs-sidebar}"]);
        assert_eq!(calls[1][0], "split-window");
        assert_eq!(calls[1][6], "@5");
        let fits = t.calls_to("resize-pane");
        assert_eq!(fits.len(), 3);
        let t = reply("", &[]);
        layout(None, &t);
        assert!(t.calls_to("split-window").is_empty());
        assert_eq!(t.calls_to("resize-pane").len(), 3);
    }

    #[test]
    fn layout_closes_a_window_where_only_the_sidebar_is_left() {
        let t = reply("", &[]);
        layout(None, &t);
        assert_eq!(t.calls_to("kill-window"), [["kill-window", "-t", "@4"]]);
    }

    #[test]
    fn layout_tells_the_panes_on_screen_that_the_layout_changed() {
        let t = reply("", &["@1", "@2"]);
        layout(None, &t);
        assert_eq!(sent(&t), [pair("%1", LAYOUT), pair("%2", LAYOUT)]);
        // Once, then again after closing @4
        assert_eq!(listings(&t), 2);
    }

    /// The rows of one window 185 wide and 47 high, each pane as `(id, sidebar, [left, top,
    /// width, height])`.
    fn team_rows(landing: &str, zoomed: &str, panes: &[(&str, &str, [u32; 4])]) -> String {
        panes
            .iter()
            .enumerate()
            .map(|(i, (id, sidebar, [l, t, w, h]))| {
                let place = format!("0\t185\t47\t{zoomed}\t{l}\t{t}\t{w}\t{h}");
                format!("{place}\t{id}\t@7\t{i}\t7\t{sidebar}\t{landing}\tteam")
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
            ["list-panes", "-a", "-F", f] if f.starts_with(PLACE_FORMAT) => rows.clone(),
            _ => String::new(),
        })
    }

    #[test]
    fn a_team_keeps_the_main_pane_big_and_stacks_the_rest() {
        let listed = parse(&team_rows("", "0", &STACKED));
        let found = teams(&listed);
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
        assert_eq!(listings(&t), 1);
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
        assert!(teams(&parse(&team_rows("", "0", &STACKED[..3]))).is_empty());
        assert!(teams(&parse(&team_rows("", "1", &STACKED))).is_empty());
        assert!(teams(&parse(&team_rows("1", "0", &STACKED))).is_empty());
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
        let t = reply("", &[]);
        step("@3", 1, &records, "/s", &t);
        assert_eq!(t.calls_to("select-window"), [["select-window", "-t", "@2"]]);
        let t = reply("", &[]);
        step("@3", -1, &records, "/s", &t);
        assert_eq!(t.calls_to("select-window"), [["select-window", "-t", "@2"]]);
    }

    #[test]
    fn step_without_sessions_selects_nothing() {
        let t = reply("", &[]);
        step("@3", 1, &[], "/s", &t);
        assert!(t.calls_to("select-window").is_empty());
    }

    #[test]
    fn show_hides_the_window_shown_before_and_shows_this_one() {
        let t = reply("@3", &[]);
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
        let t = reply("@2", &[]);
        show("%2", &t);
        assert_eq!(sent(&t), [pair("%2", SHOWN)]);
        assert!(t.calls_to("set-option").is_empty());
    }

    #[test]
    fn show_wakes_the_landing_page_and_skips_a_window_with_neither() {
        let t = reply("", &[]);
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
        let t = reply("@3", &[]);
        show("%99", &t);
        assert!(sent(&t).is_empty());
        assert!(t.calls_to("set-option").is_empty());
    }
}
