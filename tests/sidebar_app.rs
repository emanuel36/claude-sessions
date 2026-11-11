//! `cst sidebar` running in a pane of a private tmux server.

mod support;

use std::fs;
use std::ops::Deref;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cst::core::paths::write_atomic;
use cst::core::state::{Mux, Record, Status};
use cst::core::store::process_start;
use support::TmuxApp;

/// How long the sidebar gets to start and draw its first frame.
const START: Duration = Duration::from_secs(5);

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// A private server with two session windows, `alpha` and `beta`, each running `sleep`, and a
/// third window, current, running `cst sidebar`.
struct Sidebar(TmuxApp);

impl Deref for Sidebar {
    type Target = TmuxApp;

    fn deref(&self) -> &TmuxApp {
        &self.0
    }
}

impl Sidebar {
    fn start() -> Option<Self> {
        let (tmux, env) = TmuxApp::server()?;
        tmux.run(&["new-window", "-d", "-t", "t:", "-n", "beta", "sleep 600"]);
        Some(Sidebar(TmuxApp::run(tmux, env, "side", "sidebar")))
    }

    /// The pane id of the first pane of a window, by name.
    fn pane_of(&self, window: &str) -> String {
        self.tmux
            .run(&[
                "display-message",
                "-p",
                "-t",
                &format!("t:{window}"),
                "#{pane_id}",
            ])
            .trim()
            .to_owned()
    }

    /// Writes the record of a session in the first pane of `window`, run by this test process
    /// (so the sidebar finds it alive).
    fn record(&self, id: &str, window: &str, status: Status) {
        let pid = i32::try_from(std::process::id()).expect("pid");
        let at = now() - 5.0;
        let record = Record {
            id: id.to_owned(),
            status,
            cwd: format!("/w/{window}"),
            pid: Some(pid),
            pid_start: process_start(pid),
            status_since: at,
            updated_at: at,
            started_at: at,
            mux: Some(Mux {
                socket: self.tmux.socket_path().display().to_string(),
                pane: self.pane_of(window),
            }),
            ..Record::default()
        };
        let path = self.env.runtime_dir().join(format!("{id}.json"));
        let json = serde_json::to_vec(&record).expect("record json");
        write_atomic(&path, &json, None).expect("write record");
    }
}

/// User plus system CPU time of a process, in clock ticks.
fn cpu_ticks(pid: &str) -> u64 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).expect("stat");
    // Fields 14 and 15, counted after the command name in parentheses
    let rest = &stat[stat.rfind(')').expect("comm") + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    fields[11].parse::<u64>().expect("utime") + fields[12].parse::<u64>().expect("stime")
}

/// The children of every thread of a process.
fn children(pid: &str) -> Vec<String> {
    let mut found = Vec::new();
    for task in fs::read_dir(format!("/proc/{pid}/task")).expect("tasks") {
        let path = task.expect("task").path().join("children");
        let listed = fs::read_to_string(path).unwrap_or_default();
        found.extend(listed.split_whitespace().map(str::to_owned));
    }
    found
}

/// Context switches of every thread of a process: how often any of them woke up.
fn switches(pid: &str) -> u64 {
    let mut total = 0;
    for task in fs::read_dir(format!("/proc/{pid}/task")).expect("tasks") {
        let status = fs::read_to_string(task.expect("task").path().join("status"));
        for line in status.unwrap_or_default().lines() {
            if let Some((key, value)) = line.split_once(':') {
                if key.ends_with("ctxt_switches") {
                    total += value.trim().parse::<u64>().unwrap_or(0);
                }
            }
        }
    }
    total
}

#[test]
fn sidebar_draws_sessions_of_its_server() {
    let Some(side) = Sidebar::start() else { return };
    side.record("s1", "alpha", Status::Idle);
    side.record("s2", "beta", Status::Permission);
    side.assert_shows("alpha", START);
    side.assert_shows("beta", START);
}

#[test]
fn hidden_sidebar_spawns_nothing() {
    let Some(side) = Sidebar::start() else { return };
    // A working session animates the spinner while the sidebar is shown
    side.record("s1", "alpha", Status::Working);
    side.assert_shows("alpha", START);
    let pid = side.pid();
    side.send("F11");
    thread::sleep(Duration::from_millis(300));
    let (cpu, woke, screen) = (cpu_ticks(&pid), switches(&pid), side.screen());
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        assert_eq!(
            children(&pid),
            Vec::<String>::new(),
            "a hidden sidebar spawned"
        );
        thread::sleep(Duration::from_millis(100));
    }
    assert!(cpu_ticks(&pid) <= cpu + 1, "a hidden sidebar used CPU");
    assert_eq!(switches(&pid), woke, "a hidden sidebar woke up");
    assert_eq!(side.screen(), screen, "a hidden sidebar redrew");
}

#[test]
fn shown_sidebar_redraws_on_record_change() {
    let Some(side) = Sidebar::start() else { return };
    side.record("s1", "alpha", Status::Idle);
    side.assert_shows("alpha", START);
    assert!(!side.screen().contains("! alpha"));
    side.record("s1", "alpha", Status::Permission);
    side.assert_shows("! alpha", Duration::from_millis(500));
}

#[test]
fn hidden_sidebar_catches_up_when_shown() {
    let Some(side) = Sidebar::start() else { return };
    side.record("s1", "alpha", Status::Idle);
    side.assert_shows("alpha", START);
    side.send("F11");
    thread::sleep(Duration::from_millis(200));
    side.record("s1", "alpha", Status::Permission);
    assert!(
        !side.shows_within("! alpha", Duration::from_millis(500)),
        "a hidden sidebar read a record"
    );
    side.send("F12");
    side.assert_shows("! alpha", Duration::from_millis(500));
}

#[test]
fn layout_signal_picks_up_a_new_window() {
    let Some(side) = Sidebar::start() else { return };
    side.record("s1", "alpha", Status::Idle);
    side.assert_shows("alpha", START);
    side.tmux
        .run(&["new-window", "-d", "-t", "t:", "-n", "gamma", "sleep 600"]);
    side.record("s3", "gamma", Status::Idle);
    // The panes are listed again only when tmux says the layout changed
    assert!(
        !side.shows_within("gamma", Duration::from_millis(500)),
        "the sidebar listed panes without a layout signal"
    );
    side.send("F10");
    side.assert_shows("gamma", Duration::from_secs(1));
}

#[test]
fn click_selects_that_window() {
    let Some(side) = Sidebar::start() else { return };
    side.record("s1", "alpha", Status::Idle);
    side.record("s2", "beta", Status::Idle);
    side.assert_shows("beta", START);
    // alpha takes rows 5 and 6, a blank row, then beta on rows 8 and 9 (0-based); a press and
    // a release on row 8, column 4, in SGR encoding (1-based)
    side.tmux.run(&[
        "send-keys",
        "-l",
        "-t",
        &side.pane,
        "\x1b[<0;5;9M\x1b[<0;5;9m",
    ]);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let current = side
            .tmux
            .run(&["display-message", "-p", "-t", "t:", "#{window_name}"]);
        if current.trim() == "beta" {
            break;
        }
        assert!(Instant::now() < deadline, "still on {current:?}");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn records_dir_recreated_is_watched_again() {
    let Some(side) = Sidebar::start() else { return };
    side.record("s1", "alpha", Status::Idle);
    side.assert_shows("alpha", START);
    fs::remove_dir_all(side.env.runtime_dir()).expect("remove records dir");
    fs::create_dir_all(side.env.runtime_dir()).expect("create records dir");
    thread::sleep(Duration::from_millis(200));
    side.record("s1", "alpha", Status::Permission);
    side.assert_shows("! alpha", Duration::from_secs(1));
}
