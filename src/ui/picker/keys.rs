//! The folder list's keys: the key presses it takes, those that end a panel, and opening the
//! row picked.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::core::paths;
use crate::term::tmux::Runner;
use crate::ui::picker;

/// Whether a key ends a panel: Ctrl+C or Ctrl+Q.
pub fn quits(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'q'))
}

/// The key the folder list takes for a key press, if any: arrows, Enter, Backspace, Ctrl+n and
/// Ctrl+o for the entries, and a character typed with no modifier but Shift.
pub fn picker_key(key: &KeyEvent) -> Option<picker::Key> {
    let ctrl = key.modifiers == KeyModifiers::CONTROL;
    Some(match key.code {
        KeyCode::Char('n') if ctrl => picker::Key::Entry(picker::Root::New),
        KeyCode::Char('o') if ctrl => picker::Key::Entry(picker::Root::Open),
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
///
/// A create row's folder, and any missing parent, is created first, as is the folder of a
/// session found that was deleted; when that fails, the failure goes to the error log and
/// nothing starts.
pub fn open(tmux: &dyn Runner, row: &picker::Row) {
    open_with(tmux, row, &paths::process_env);
}

/// [`open`], logging to the state directory `env` names.
fn open_with(tmux: &dyn Runner, row: &picker::Row, env: paths::Lookup<'_>) {
    if let picker::Row::Create { path, .. } | picker::Row::Hit { path, .. } = row {
        if let Err(e) = std::fs::create_dir_all(path) {
            paths::log_error_with(env, &format_args!("cannot create {path}: {e}"));
            return;
        }
    }
    let Some(command) = picker::command(row) else {
        return;
    };
    let mut args = vec!["new-window"];
    args.extend(command.iter().map(String::as_str));
    tmux.run(&args);
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::core::paths::ERROR_LOG;
    use crate::term::tmux::fake::FakeTmux;
    use crate::ui::picker::keys::{open, open_with, picker_key, quits};
    use crate::ui::picker::{Key, Root, Row};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
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
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(picker_key(&ctrl('n')), Some(Key::Entry(Root::New)));
        assert_eq!(picker_key(&ctrl('o')), Some(Key::Entry(Root::Open)));
        assert_eq!(picker_key(&key(KeyCode::Tab)), None);
        assert_eq!(picker_key(&key(KeyCode::Esc)), None);
    }

    /// A row that creates the folder `path`, then starts a session in it.
    fn create_row(path: &std::path::Path) -> Row {
        Row::Create {
            root: Root::New,
            path: path.to_string_lossy().into_owned(),
            label: String::new(),
        }
    }

    /// A session found in the folder `path`, whose id is `id`.
    fn hit_row(path: &std::path::Path) -> Row {
        Row::Hit {
            index: 0,
            path: path.to_string_lossy().into_owned(),
            id: "id".into(),
            more_below: false,
        }
    }

    #[test]
    fn open_creates_the_folder_then_starts_there() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("Documents/Meu-Projeto");
        let tmux = FakeTmux::silent();
        open(&tmux, &create_row(&path));
        assert!(path.is_dir());
        let want = ["new-window", "-c", &path.to_string_lossy(), "claude"];
        assert_eq!(tmux.calls(), [want.map(str::to_owned).to_vec()]);
    }

    #[test]
    fn open_logs_and_starts_nothing_when_create_fails() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("x");
        std::fs::write(&path, "").unwrap();
        let state = t.path().join("state");
        let env = |key: &str| (key == "CST_STATE_DIR").then(|| state.clone().into_os_string());
        let tmux = FakeTmux::silent();
        open_with(&tmux, &create_row(&path), &env);
        assert!(tmux.calls().is_empty());
        let log = std::fs::read_to_string(state.join(ERROR_LOG)).unwrap();
        assert!(log.contains(&*path.to_string_lossy()), "{log}");
    }

    #[test]
    fn a_hit_in_a_deleted_folder_creates_it_and_resumes() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("gone/Projeto");
        let tmux = FakeTmux::silent();
        open(&tmux, &hit_row(&path));
        assert!(path.is_dir());
        let want = [
            "new-window",
            "-c",
            &path.to_string_lossy(),
            "claude --resume id",
        ];
        assert_eq!(tmux.calls(), [want.map(str::to_owned).to_vec()]);
    }

    #[test]
    fn a_hit_logs_and_starts_nothing_when_its_folder_cannot_be_made() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("x");
        std::fs::write(&path, "").unwrap();
        let state = t.path().join("state");
        let env = |key: &str| (key == "CST_STATE_DIR").then(|| state.clone().into_os_string());
        let tmux = FakeTmux::silent();
        open_with(&tmux, &hit_row(&path), &env);
        assert!(tmux.calls().is_empty());
        let log = std::fs::read_to_string(state.join(ERROR_LOG)).unwrap();
        assert!(log.contains(&*path.to_string_lossy()), "{log}");
    }
}
