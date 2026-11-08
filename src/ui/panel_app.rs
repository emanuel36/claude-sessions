//! The Alt+n panel, `cst new --pick`: the folder list that starts a session, in the popup tmux
//! opens for Alt+n.
//!
//! It draws its search line first, then reads the history, which takes a moment when long. It
//! ends on Esc, Ctrl+C or Ctrl+Q, and once Enter opened a window.

use std::collections::HashSet;

use anyhow::Context;
use crossterm::event::KeyCode;
use ratatui::widgets::Paragraph;

use crate::claude::history::HistoryReader;
use crate::claude::hook;
use crate::claude::transcript::Titles;
use crate::core::state::unix_now;
use crate::term::tmux::{Cli, Runner};
use crate::ui::folders::Scan;
use crate::ui::pane::{self, open, picker_key, quits, App, LogOnce, Step};
use crate::ui::picker::{self, Folder, Picker};
use crate::ui::runtime::{Msg, Runtime, Schedule, Screen};
use crate::ui::usage::Reader;

/// Runs the panel in the terminal this process runs in, until the user leaves it.
///
/// # Errors
///
/// Fails when the terminal or the event loop cannot be set up.
pub fn main_panel() -> anyhow::Result<()> {
    let mut runtime = Runtime::start().context("cannot start the event loop")?;
    let mut screen = Screen::enter().context("cannot set the terminal up")?;
    let mut app = Panel::new(Box::new(Cli::default()), Picker::default());
    // Reading the history takes a moment on a long one: the search line comes first
    app.update(unix_now(), &mut runtime, &mut screen);
    app.picker.folders = folders(unix_now());
    app.update(unix_now(), &mut runtime, &mut screen);
    pane::run(&mut app, &mut runtime, &mut screen);
    Ok(())
}

/// The folders Claude ran in that still exist, last used first; the history and the titles
/// read are kept on disk for the next panel.
fn folders(now: f64) -> Vec<Folder> {
    let mut history = HistoryReader::new(true);
    history.load();
    let generation = history.generation();
    let mut usage = Reader::new();
    let costs = &usage
        .totals(history.sessions(), generation, now)
        .per_session;
    let open: HashSet<String> = hook::live_sessions(now).into_iter().map(|r| r.id).collect();
    Scan::new(Titles::new(true)).load(history.sessions(), generation, costs, &open)
}

/// The panel: the folder list, and the server it opens windows in.
struct Panel {
    tmux: Box<dyn Runner>,
    picker: Picker,
    /// Never wakes the panel by itself: it changes only when the user types.
    schedule: Schedule,
    draw_error: LogOnce,
}

impl Panel {
    fn new(tmux: Box<dyn Runner>, picker: Picker) -> Self {
        Panel {
            tmux,
            picker,
            schedule: Schedule::default(),
            draw_error: LogOnce::new("new"),
        }
    }
}

impl App for Panel {
    fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    fn handle(&mut self, msg: Msg, _now: f64, _runtime: &mut Runtime) -> Step {
        match msg {
            Msg::Key(key) if quits(&key) || key.code == KeyCode::Esc => Step::Quit,
            Msg::Key(key) => {
                let Some(typed) = picker_key(&key) else {
                    return Step::Wait;
                };
                match self.picker.key(typed) {
                    Some(row) => {
                        open(self.tmux.as_ref(), &row, &self.picker.folders);
                        Step::Quit
                    }
                    None => Step::Draw,
                }
            }
            Msg::Resize => Step::Draw,
            // Clicks do nothing, and nothing is watched
            Msg::Mouse(_) | Msg::Changed(_) => Step::Wait,
        }
    }

    fn update(&mut self, now: f64, _runtime: &mut Runtime, screen: &mut Screen) {
        let drawn = screen.draw(|frame| {
            let area = frame.area();
            let lines = picker::screen(&mut self.picker, now, area.height);
            frame.render_widget(Paragraph::new(lines), area);
        });
        if let Err(e) = drawn {
            self.draw_error.log(&format_args!("cannot draw: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::term::tmux::Runner;
    use crate::ui::pane::{App, Step};
    use crate::ui::panel_app::Panel;
    use crate::ui::picker::{Folder, Picker, RowKind, Session};
    use crate::ui::runtime::{Msg, Runtime};

    const NOW: f64 = 1_700_000_000.0;

    /// Records the tmux commands it is asked to run.
    #[derive(Clone, Default)]
    struct Calls(Rc<RefCell<Vec<Vec<String>>>>);

    impl Calls {
        fn all(&self) -> Vec<Vec<String>> {
            self.0.borrow().clone()
        }
    }

    impl Runner for Calls {
        fn run(&self, args: &[&str]) -> String {
            let args = args.iter().map(|a| (*a).to_owned()).collect();
            self.0.borrow_mut().push(args);
            String::new()
        }
    }

    fn session(id: &str, title: &str, last: f64) -> Session {
        Session {
            id: id.into(),
            title: title.into(),
            last,
            cost: None,
        }
    }

    fn folders() -> Vec<Folder> {
        vec![
            Folder {
                path: "/w/api".into(),
                name: "api".into(),
                last: NOW,
                count: 5,
                spend: 0.0,
                sessions: vec![session("s1", "fix it", NOW - 60.0), session("s2", "x", 0.0)],
            },
            Folder {
                path: "/w/web".into(),
                name: "web".into(),
                last: NOW - 9.0,
                count: 2,
                spend: 0.0,
                sessions: Vec::new(),
            },
        ]
    }

    /// A panel over two folders, `/w/api` and `/w/web` being the only folders on disk; the
    /// steps of its last key and its tmux calls.
    fn drive(codes: &[KeyCode]) -> (Panel, Vec<Step>, Calls) {
        let calls = Calls::default();
        let mut picker = Picker::new(Box::new(|p| p == "/w/api" || p == "/w/web"));
        picker.folders = folders();
        let mut panel = Panel::new(Box::new(calls.clone()), picker);
        let mut runtime = Runtime::headless().unwrap();
        let steps = codes
            .iter()
            .map(|&code| {
                let key = KeyEvent::new(code, KeyModifiers::NONE);
                panel.handle(Msg::Key(key), NOW, &mut runtime)
            })
            .collect();
        (panel, steps, calls)
    }

    fn chars(text: &str) -> Vec<KeyCode> {
        text.chars().map(KeyCode::Char).collect()
    }

    #[test]
    fn enter_on_a_folder_starts_a_session_there() {
        let (_, steps, calls) = drive(&[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(calls.all(), [["new-window", "-c", "/w/web", "claude"]]);
        assert_eq!(steps, [Step::Draw, Step::Quit]);
    }

    #[test]
    fn right_opens_the_sessions_and_enter_resumes_one() {
        let (_, _, calls) = drive(&[KeyCode::Right, KeyCode::Down, KeyCode::Enter]);
        assert_eq!(
            calls.all(),
            [["new-window", "-c", "/w/api", "claude --resume s1"]]
        );
    }

    #[test]
    fn left_from_a_session_goes_back_to_its_folder_and_closes_it() {
        let keys = [KeyCode::Right, KeyCode::Down, KeyCode::Down, KeyCode::Left];
        let (panel, _, _) = drive(&keys);
        assert_eq!(panel.picker.cursor, 0);
        assert!(panel.picker.expanded.is_empty());
    }

    #[test]
    fn typing_filters_and_a_typed_path_opens_there() {
        let mut keys = chars("web");
        keys.push(KeyCode::Enter);
        let (_, _, calls) = drive(&keys);
        assert_eq!(calls.all(), [["new-window", "-c", "/w/web", "claude"]]);
        let mut keys = chars("/w/api");
        keys.push(KeyCode::Enter);
        let (_, _, calls) = drive(&keys);
        assert_eq!(calls.all(), [["new-window", "-c", "/w/api", "claude"]]);
    }

    #[test]
    fn left_returns_to_the_folder_even_when_a_typed_path_matches_it() {
        let mut keys = chars("/w/api");
        keys.extend([KeyCode::Down, KeyCode::Right, KeyCode::Down, KeyCode::Left]);
        let (mut panel, _, _) = drive(&keys);
        assert_eq!(panel.picker.selected().unwrap().kind, RowKind::Folder);
        assert!(panel.picker.expanded.is_empty());
    }

    #[test]
    fn escape_closes_without_starting_anything() {
        let (_, steps, calls) = drive(&[KeyCode::Esc]);
        assert_eq!(steps, [Step::Quit]);
        assert_eq!(calls.all(), Vec::<Vec<String>>::new());
    }

    #[test]
    fn ctrl_c_and_ctrl_q_close_without_starting_anything() {
        for c in ['c', 'q'] {
            let (mut panel, _, calls) = drive(&[]);
            let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
            let mut runtime = Runtime::headless().unwrap();
            assert_eq!(panel.handle(Msg::Key(key), NOW, &mut runtime), Step::Quit);
            assert_eq!(calls.all(), Vec::<Vec<String>>::new());
        }
    }

    #[test]
    fn enter_with_nothing_listed_stays() {
        let mut keys = chars("zzz");
        keys.push(KeyCode::Enter);
        let (_, steps, calls) = drive(&keys);
        assert_eq!(steps.last(), Some(&Step::Draw));
        assert_eq!(calls.all(), Vec::<Vec<String>>::new());
    }

    #[test]
    fn a_resize_redraws() {
        let (mut panel, _, _) = drive(&[]);
        let mut runtime = Runtime::headless().unwrap();
        assert_eq!(panel.handle(Msg::Resize, NOW, &mut runtime), Step::Draw);
    }
}
