//! What the sidebar, the landing page and the Alt+n panel share: the loop they run on, how they
//! learn whether they are on screen and how they log failures. What they watch is in
//! [`crate::ui::watch`].

use std::fmt::Display;
use std::thread;
use std::time::Duration;

use anyhow::bail;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::core::paths;
use crate::core::state::unix_now;
use crate::term::tmux::{self, Runner};
use crate::ui::picker;
use crate::ui::runtime::{signal, Msg, Runtime, Schedule, Screen, Signal, Wake};

/// How many times tmux is asked for the pane's window before giving up.
const WINDOW_TRIES: u32 = 5;
/// The pause between two of those asks.
pub const WINDOW_PAUSE: Duration = Duration::from_millis(100);

/// What a pane does after taking a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Waits for more; a redraw comes when the schedule says.
    Wait,
    /// Reads what is stale and draws at once.
    Draw,
    /// Ends the loop.
    Quit,
}

/// A pane run by [`run`].
pub trait App {
    /// When the pane must wake up with no event.
    fn schedule(&self) -> &Schedule;
    /// Takes one message.
    fn handle(&mut self, msg: Msg, now: f64, runtime: &mut Runtime) -> Step;
    /// Reads what is stale, then draws.
    fn update(&mut self, now: f64, runtime: &mut Runtime, screen: &mut Screen);
}

/// Runs a pane until it quits or its terminal goes away.
///
/// A burst of events draws at once, then at most every 100 ms: events only mark what is
/// stale, and the reading happens in [`App::update`].
pub fn run(app: &mut impl App, runtime: &mut Runtime, screen: &mut Screen) {
    loop {
        let deadline = app.schedule().next_wake(unix_now());
        let wake = runtime.wait(deadline);
        let now = unix_now();
        let at_once = match wake {
            Wake::Msg(msg) => match app.handle(msg, now, runtime) {
                Step::Quit => return,
                Step::Draw => true,
                Step::Wait => false,
            },
            Wake::Timeout => true,
            // The terminal is gone: nobody sees the pane any more
            Wake::Closed => return,
        };
        if at_once || app.schedule().next_wake(now).is_some_and(|at| at <= now) {
            app.update(now, runtime, screen);
        }
    }
}

/// What a message tells about whether the pane is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sight {
    /// It is on screen now.
    Shown,
    /// It left the screen.
    Hidden,
    /// Nothing new.
    Same,
}

/// What `msg` tells a pane that believes itself `visible` about being on screen.
///
/// tmux says so with F12 and F11. A pane believed hidden that gets a key the user typed, a
/// click, a resize or a layout change is on screen all the same: tmux's word was lost.
///
/// A resize is such a proof because tmux (3.1 on; tmux.conf requires 3.3) resizes a window
/// only when it is current or becomes current, never while it is out of sight.
pub fn sight(msg: &Msg, visible: bool) -> Sight {
    match msg {
        Msg::Key(key) => match signal(key) {
            Some(Signal::Shown) => Sight::Shown,
            Some(Signal::Hidden) => Sight::Hidden,
            Some(Signal::Layout) | None if !visible => Sight::Shown,
            Some(Signal::Layout) | None => Sight::Same,
        },
        Msg::Mouse(_) | Msg::Resize if !visible => Sight::Shown,
        Msg::Mouse(_) | Msg::Resize | Msg::Changed(_) => Sight::Same,
    }
}

/// A pane tmux shows and hides: the sidebar and the landing page.
pub trait Visible {
    /// Whether the pane believes itself on screen.
    fn visible(&self) -> bool;
    /// Comes on screen: everything is read again before the next draw.
    fn show(&mut self);
    /// Leaves the screen: no watches, no timer, nothing read until shown again.
    fn hide(&mut self, runtime: &mut Runtime);
}

/// Takes a message for a pane tmux shows and hides; `take` handles it once the pane is known
/// to be on screen.
///
/// The pane is shown or hidden as the message tells ([`sight`]). A hidden pane does no work:
/// the message that hid it, and any that comes while hidden (such as changes queued before
/// its watches went), is dropped. A pane just shown draws at once, whatever `take` returns.
pub fn gated<A: Visible>(
    app: &mut A,
    msg: Msg,
    runtime: &mut Runtime,
    take: impl FnOnce(&mut A, Msg, &mut Runtime) -> Step,
) -> Step {
    match sight(&msg, app.visible()) {
        Sight::Shown => {
            app.show();
            match take(app, msg, runtime) {
                Step::Quit => Step::Quit,
                Step::Wait | Step::Draw => Step::Draw,
            }
        }
        Sight::Hidden => {
            app.hide(runtime);
            Step::Wait
        }
        Sight::Same if app.visible() => take(app, msg, runtime),
        Sight::Same => Step::Wait,
    }
}

/// The tmux window a pane is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnWindow {
    /// The window's id, as `@3`.
    pub id: String,
    /// Whether it is its session's current window; a pane started in another one starts out
    /// of sight.
    pub current: bool,
}

/// The tmux window `pane` is in, asking tmux up to five times, `pause` apart.
///
/// tmux can be slow to answer a pane it has just started, as on a busy server.
///
/// # Errors
///
/// Fails when `pane` is `None` or empty (not in tmux) or tmux never answers.
pub fn own_window(
    tmux: &dyn Runner,
    pane: Option<&str>,
    pause: Duration,
) -> anyhow::Result<OwnWindow> {
    let Some(pane) = pane.filter(|p| !p.is_empty()) else {
        bail!("not in a tmux pane: TMUX_PANE is not set");
    };
    let format = "#{window_id}\t#{window_active}";
    for attempt in 0..WINDOW_TRIES {
        if attempt > 0 {
            thread::sleep(pause);
        }
        let out = tmux.run(&["display-message", "-p", "-t", pane, format]);
        if let Some((id, active)) = out.split_once('\t').filter(|(id, _)| !id.is_empty()) {
            return Ok(OwnWindow {
                id: id.to_owned(),
                current: active == "1",
            });
        }
    }
    bail!("cannot find the window of pane {pane}")
}

/// The socket of the tmux server this process runs in; records name their server by it.
pub fn own_socket() -> String {
    std::env::var("TMUX")
        .ok()
        .and_then(|t| tmux::socket_of(&t).map(str::to_owned))
        .unwrap_or_default()
}

/// Whether a key ends a panel: Ctrl+C or Ctrl+Q.
pub fn quits(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'q'))
}

/// The key the folder list takes for a key press, if any: arrows, Enter, Backspace, and a
/// character typed with no modifier but Shift.
pub fn picker_key(key: &KeyEvent) -> Option<picker::Key> {
    Some(match key.code {
        KeyCode::Up => picker::Key::Up,
        KeyCode::Down => picker::Key::Down,
        KeyCode::Left => picker::Key::Left,
        KeyCode::Right => picker::Key::Right,
        KeyCode::Enter => picker::Key::Enter,
        KeyCode::Backspace => picker::Key::Backspace,
        KeyCode::Char(c) if (key.modifiers - KeyModifiers::SHIFT).is_empty() => {
            picker::Key::Char(c)
        }
        _ => return None,
    })
}

/// Runs `new-window` for a row of the folder list: resume its session, or start one.
pub fn open(tmux: &dyn Runner, row: &picker::Row, folders: &[picker::Folder]) {
    let command = picker::command(row, folders);
    let mut args = vec!["new-window"];
    args.extend(command.iter().map(String::as_str));
    tmux.run(&args);
}

/// Logs a failure unless it repeats the last one, which would fill the log on every redraw.
#[derive(Debug)]
pub struct LogOnce {
    /// The pane, named before each line.
    pane: &'static str,
    last: Option<String>,
}

impl LogOnce {
    /// Logs for the pane called `pane`.
    pub fn new(pane: &'static str) -> Self {
        LogOnce { pane, last: None }
    }

    /// Returns the line to log, or `None` when it is the same as the last failure.
    fn check(&mut self, what: &dyn Display) -> Option<String> {
        let line = what.to_string();
        if self.last.as_ref() == Some(&line) {
            return None;
        }
        self.last = Some(line.clone());
        Some(line)
    }

    /// Writes a failure to the error log, unless it repeats the last one.
    pub fn log(&mut self, what: &dyn Display) {
        if let Some(line) = self.check(what) {
            paths::log_error(&format_args!("{}: {line}", self.pane));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::time::Duration;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    use crate::term::tmux::fake::FakeTmux;
    use crate::ui::pane::{own_window, picker_key, quits, sight, LogOnce, Sight};
    use crate::ui::picker::Key;
    use crate::ui::runtime::Msg;

    /// Answers each call with the next of `answers`, then with nothing.
    fn answering(answers: &[&'static str]) -> FakeTmux {
        let left = RefCell::new(answers.iter().rev().copied().collect::<Vec<_>>());
        FakeTmux::new(move |_| left.borrow_mut().pop().unwrap_or_default().to_owned())
    }

    /// How many times `tmux` was asked for pane `%7`'s window, failing on any other call.
    fn asks(tmux: &FakeTmux) -> usize {
        let ask = [
            "display-message",
            "-p",
            "-t",
            "%7",
            "#{window_id}\t#{window_active}",
        ];
        let calls = tmux.calls();
        assert!(calls.iter().all(|c| c == &ask), "{calls:?}");
        calls.len()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn click() -> Msg {
        Msg::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 4,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn own_window_asks_again_until_tmux_answers() {
        let tmux = answering(&["", "", "@3\t1"]);
        let got = own_window(&tmux, Some("%7"), Duration::ZERO).unwrap();
        assert_eq!((got.id.as_str(), got.current, asks(&tmux)), ("@3", true, 3));
    }

    #[test]
    fn own_window_tells_a_window_out_of_sight() {
        let tmux = answering(&["@3\t0"]);
        let got = own_window(&tmux, Some("%7"), Duration::ZERO).unwrap();
        assert_eq!((got.id.as_str(), got.current), ("@3", false));
    }

    #[test]
    fn own_window_gives_up_after_five_asks() {
        let tmux = answering(&[]);
        let err = own_window(&tmux, Some("%7"), Duration::ZERO).unwrap_err();
        assert_eq!(asks(&tmux), 5);
        assert!(err.to_string().contains("%7"), "{err}");
    }

    #[test]
    fn own_window_needs_a_pane() {
        let tmux = answering(&["@3"]);
        assert!(own_window(&tmux, None, Duration::ZERO).is_err());
        assert!(own_window(&tmux, Some(""), Duration::ZERO).is_err());
        assert_eq!(asks(&tmux), 0);
    }

    #[test]
    fn tmux_signals_show_and_hide() {
        for visible in [true, false] {
            let f = |n| Msg::Key(key(KeyCode::F(n)));
            assert_eq!(sight(&f(12), visible), Sight::Shown);
            assert_eq!(sight(&f(11), visible), Sight::Hidden);
        }
    }

    #[test]
    fn what_only_a_pane_on_screen_gets_shows_a_hidden_one() {
        let shows = [
            click(),
            Msg::Resize,
            Msg::Key(key(KeyCode::F(10))),
            Msg::Key(key(KeyCode::Char('a'))),
            Msg::Key(key(KeyCode::Up)),
        ];
        for msg in &shows {
            assert_eq!(sight(msg, false), Sight::Shown, "{msg:?}");
            assert_eq!(sight(msg, true), Sight::Same, "{msg:?}");
        }
    }

    #[test]
    fn a_change_on_disk_does_not_show_a_hidden_pane() {
        // Queued before the watches went
        let changed = Msg::Changed(PathBuf::from("/run/cst/s1.json"));
        assert_eq!(sight(&changed, false), Sight::Same);
    }

    #[test]
    fn ctrl_c_and_ctrl_q_quit() {
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert!(quits(&ctrl('c')) && quits(&ctrl('q')));
        assert!(!quits(&key(KeyCode::Char('q'))) && !quits(&ctrl('x')));
    }

    #[test]
    fn keys_map_to_the_folder_list() {
        assert_eq!(picker_key(&key(KeyCode::Up)), Some(Key::Up));
        assert_eq!(picker_key(&key(KeyCode::Down)), Some(Key::Down));
        assert_eq!(picker_key(&key(KeyCode::Left)), Some(Key::Left));
        assert_eq!(picker_key(&key(KeyCode::Right)), Some(Key::Right));
        assert_eq!(picker_key(&key(KeyCode::Enter)), Some(Key::Enter));
        assert_eq!(picker_key(&key(KeyCode::Backspace)), Some(Key::Backspace));
        assert_eq!(picker_key(&key(KeyCode::Char('/'))), Some(Key::Char('/')));
        let shifted = KeyEvent::new(KeyCode::Char('W'), KeyModifiers::SHIFT);
        assert_eq!(picker_key(&shifted), Some(Key::Char('W')));
        let ctrl = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        let alt = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT);
        assert_eq!((picker_key(&ctrl), picker_key(&alt)), (None, None));
        assert_eq!(picker_key(&key(KeyCode::Tab)), None);
        assert_eq!(picker_key(&key(KeyCode::Esc)), None);
    }

    #[test]
    fn a_failure_is_logged_once_until_another_one() {
        let mut log = LogOnce::new("sidebar");
        assert_eq!(log.check(&"x: gone"), Some("x: gone".to_owned()));
        assert_eq!(log.check(&"x: gone"), None);
        assert_eq!(log.check(&"y: bad"), Some("y: bad".to_owned()));
        assert_eq!(log.check(&"x: gone"), Some("x: gone".to_owned()));
    }

    #[test]
    fn load_and_draw_failures_are_logged_apart() {
        let (mut load, mut draw) = (LogOnce::new("sidebar"), LogOnce::new("sidebar"));
        assert!(load.check(&"x").is_some());
        assert!(draw.check(&"x").is_some());
    }
}
