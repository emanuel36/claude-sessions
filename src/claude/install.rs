//! Registers `cst` in Claude Code's settings.json, and takes it out again.
//!
//! Claude Code's settings belong to the user, so everything that is not ours is kept as it is:
//! other hooks, other keys, their order and the file's mode. Our hooks are the `cst hook` and
//! `cst statusline` commands; the old Python version's `cs hook` and `cs statusline` count as
//! ours too, so installing over it points them at `cst` (see the spec, "Migrating a Python
//! install").
//!
//! The commands are `install_at` and `uninstall_at`, which take the places they read and write
//! and the streams they print to, so unit tests never touch the process environment.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::claude::statusline;
use crate::core::paths::{self, Lookup};
use crate::shell::quote;

/// The hook events we register for.
pub const EVENTS: [&str; 11] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Notification",
    "Stop",
    "StopFailure",
    "SubagentStop",
];
/// The copy of the user's settings made before our first change, beside the settings file.
pub const BACKUP: &str = "settings.json.before-claude-sessions";
/// Seconds Claude Code waits for a hook before giving up on it.
pub const HOOK_TIMEOUT: u64 = 10;

/// The files `install` and `uninstall` read and write.
pub(crate) struct Locations {
    /// Claude Code's settings.json; a symlink is followed on writing.
    pub settings: PathBuf,
    /// The directory that holds the saved status line, created on install.
    pub state_dir: PathBuf,
    /// Where the user's own status line is saved.
    pub saved: PathBuf,
    /// Where the Python version saved it.
    pub legacy_saved: PathBuf,
}

/// Splits a command line into words like Python's `shlex.split`, or `None` if it is malformed.
fn split(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    // An empty quoted string is a word, so a word's start is not told by its length
    let mut started = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            '\\' => {
                word.push(chars.next()?);
                started = true;
            }
            '\'' => {
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
                started = true;
            }
            '"' => {
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => {
                            let escaped = chars.next()?;
                            if escaped != '"' && escaped != '\\' {
                                word.push('\\');
                            }
                            word.push(escaped);
                        }
                        c => word.push(c),
                    }
                }
                started = true;
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    Some(words)
}

fn hook_command(cst: &str) -> String {
    format!("{} hook", quote(cst))
}

fn statusline_command(cst: &str) -> String {
    format!("{} statusline", quote(cst))
}

/// Tells whether a command is `cst hook` or `cst statusline`, whichever path `cst` has.
///
/// The old Python version's `cs` counts too, so its entries are migrated and removed like ours.
pub fn is_ours(command: &str) -> bool {
    let Some(argv) = split(command) else {
        return false;
    };
    let [program, arg] = argv.as_slice() else {
        return false;
    };
    let name = program.rsplit('/').next().unwrap_or_default();
    matches!(name, "cs" | "cst") && matches!(arg.as_str(), "hook" | "statusline")
}

fn command_of(entry: &Value) -> Option<&str> {
    entry.get("command")?.as_str()
}

fn entry_is_ours_in(entry: &serde_json::Map<String, Value>) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(is_ours)
}

fn entry_is_ours(entry: &Value) -> bool {
    command_of(entry).is_some_and(is_ours)
}

fn group_has_ours(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| hooks.iter().any(entry_is_ours))
}

/// Returns what is malformed in `settings` for our purposes, or `None`.
pub fn shape_error(settings: &Value) -> Option<String> {
    let Some(root) = settings.as_object() else {
        return Some("settings is not an object".to_owned());
    };
    if let Some(hooks) = root.get("hooks") {
        let Some(hooks) = hooks.as_object() else {
            return Some("hooks is not an object".to_owned());
        };
        for (event, groups) in hooks {
            let Some(groups) = groups.as_array() else {
                return Some(format!("hooks.{event} is not a list"));
            };
            let sound = |g: &Value| g.is_object() && g.get("hooks").is_some_and(Value::is_array);
            if !groups.iter().all(sound) {
                return Some(format!("hooks.{event} has a group without a hooks list"));
            }
        }
    }
    match root.get("statusLine") {
        Some(line) if !line.is_object() => Some("statusLine is not an object".to_owned()),
        _ => None,
    }
}

/// Points our entries in `groups` at `command`. Returns whether any of them was ours.
fn repoint(groups: &mut [Value], command: &str) -> bool {
    let mut found = false;
    for group in groups {
        let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            continue;
        };
        for hook in hooks {
            if entry_is_ours(hook) {
                found = true;
                hook["command"] = json!(command);
            }
        }
    }
    found
}

/// Adds our hooks and status line to `settings`, or points ours at `cst` if it moved.
///
/// `settings` is the parsed settings.json and is not modified; `cst` is the path of the
/// executable and `saved` the user's own status line saved by an earlier install, if any.
/// Returns the new settings and the status line to save (the user's own, or `None`).
pub fn merge(settings: &Value, cst: &str, saved: Option<Value>) -> (Value, Option<Value>) {
    let mut new = settings.clone();
    let Some(root) = new.as_object_mut() else {
        return (new, saved);
    };
    let command = hook_command(cst);
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    if let Some(hooks) = hooks.as_object_mut() {
        for event in EVENTS {
            let groups = hooks.entry(event).or_insert_with(|| json!([]));
            let Some(groups) = groups.as_array_mut() else {
                continue;
            };
            if !repoint(groups, &command) {
                let entry = json!({"type": "command", "command": command, "timeout": HOOK_TIMEOUT});
                groups.push(json!({"hooks": [entry]}));
            }
        }
    }
    let command = statusline_command(cst);
    match root.get_mut("statusLine") {
        Some(Value::Object(line)) if entry_is_ours_in(line) => {
            line.insert("command".to_owned(), json!(command));
            (new, saved)
        }
        Some(Value::Object(line)) => {
            let own = Value::Object(line.clone());
            line.insert("command".to_owned(), json!(command));
            (new, Some(own))
        }
        _ => {
            root.insert(
                "statusLine".to_owned(),
                json!({"type": "command", "command": command}),
            );
            (new, None)
        }
    }
}

/// Our entries taken out of a group, or `None` if that leaves the group empty.
fn without_ours(group: &Value) -> Option<Value> {
    let mut group = group.clone();
    if let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) {
        hooks.retain(|h| !entry_is_ours(h));
        if hooks.is_empty() {
            return None;
        }
    }
    Some(group)
}

/// Takes our hooks out of `settings` and puts the user's own status line back.
///
/// An event list, or the hooks object, that only our entries filled goes away with them; an
/// empty one the user had stays. `settings` is not modified; `saved` is the user's own status
/// line saved by install, if any.
pub fn unmerge(settings: &Value, saved: Option<Value>) -> Value {
    let mut new = settings.clone();
    let Some(root) = new.as_object_mut() else {
        return new;
    };
    let mut drop_hooks = false;
    if let Some(Value::Object(hooks)) = root.get_mut("hooks") {
        let mut emptied = false;
        let events: Vec<String> = hooks.keys().cloned().collect();
        for event in events {
            let Some(Value::Array(groups)) = hooks.get(&event) else {
                continue;
            };
            if !groups.iter().any(group_has_ours) {
                continue;
            }
            let kept: Vec<Value> = groups
                .iter()
                .filter_map(|g| {
                    if group_has_ours(g) {
                        without_ours(g)
                    } else {
                        Some(g.clone())
                    }
                })
                .collect();
            if kept.is_empty() {
                hooks.shift_remove(&event);
                emptied = true;
            } else {
                hooks.insert(event, Value::Array(kept));
            }
        }
        drop_hooks = emptied && hooks.is_empty();
    }
    if drop_hooks {
        root.shift_remove("hooks");
    }
    if root.get("statusLine").is_some_and(entry_is_ours) {
        match saved {
            Some(own) => {
                root.insert("statusLine".to_owned(), own);
            }
            None => {
                root.shift_remove("statusLine");
            }
        }
    }
    new
}

/// Where the Python version saved the user's status line: `$CLAUDE_SESSIONS_STATE`, or else
/// `claude-sessions` in the XDG state directory (`~/.local/state` by default).
pub(crate) fn legacy_saved_path(env: Lookup<'_>, home: &Path) -> PathBuf {
    if let Some(dir) = paths::var(env, "CLAUDE_SESSIONS_STATE") {
        return dir.join("statusline.json");
    }
    let base = paths::var(env, "XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state"));
    base.join("claude-sessions/statusline.json")
}

fn locations() -> Locations {
    let home = paths::home(&paths::process_env);
    Locations {
        settings: paths::config_dir().join("settings.json"),
        state_dir: paths::state_dir(),
        saved: statusline::saved_statusline_path(),
        legacy_saved: legacy_saved_path(&paths::process_env, &home),
    }
}

/// What went wrong, and in which file.
struct Failure {
    path: PathBuf,
    why: String,
}

impl Failure {
    fn new(path: &Path, why: impl Into<String>) -> Self {
        Self {
            path: path.to_owned(),
            why: why.into(),
        }
    }

    fn io(path: &Path) -> impl Fn(io::Error) -> Self + '_ {
        move |e| {
            // The message without Rust's " (os error N)" suffix, as the C library words it
            let text = e.to_string();
            let why = text.split(" (os error").next().unwrap_or(&text);
            Self::new(path, why)
        }
    }
}

/// A JSON file that is missing, is not a JSON object, or is one.
enum Read {
    Missing,
    Invalid,
    Object(Value),
}

fn read_object(path: &Path) -> io::Result<Read> {
    match fs::read(path) {
        Ok(bytes) => Ok(match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) if value.is_object() => Read::Object(value),
            _ => Read::Invalid,
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Read::Missing),
        Err(e) => Err(e),
    }
}

fn dump(settings: &Value) -> io::Result<Vec<u8>> {
    let mut text = serde_json::to_string_pretty(settings).map_err(io::Error::from)?;
    text.push('\n');
    Ok(text.into_bytes())
}

/// Returns the file `path` leads to, following symlinks even where the target does not exist.
fn real_path(path: &Path) -> PathBuf {
    if let Ok(real) = fs::canonicalize(path) {
        return real;
    }
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        let Ok(target) = fs::read_link(&current) else {
            break;
        };
        // An absolute target replaces the path
        current = current.parent().unwrap_or(Path::new("")).join(target);
    }
    let parent = current.parent().and_then(|d| fs::canonicalize(d).ok());
    match (parent, current.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => current,
    }
}

/// Returns the parsed settings, `{}` if the file is missing.
fn load_settings(path: &Path) -> Result<Value, Failure> {
    let settings = match read_object(path).map_err(Failure::io(path))? {
        Read::Missing => json!({}),
        Read::Invalid => return Err(Failure::new(path, "not a valid JSON object")),
        Read::Object(settings) => settings,
    };
    match shape_error(&settings) {
        Some(problem) => Err(Failure::new(path, problem)),
        None => Ok(settings),
    }
}

/// Registers the running `cst` in Claude Code's settings.json. Returns the exit code.
///
/// Prints what changed, or `already installed`; a failure is printed to stderr and leaves
/// settings.json as it was.
pub fn install(cst: &Path) -> i32 {
    let (mut out, mut err) = (io::stdout().lock(), io::stderr().lock());
    let Some(cst) = cst.to_str() else {
        let _ = writeln!(err, "error: {}: not a valid UTF-8 path", cst.display());
        return 1;
    };
    install_at(&locations(), cst, &mut out, &mut err)
}

/// Checks that `install` would succeed, and writes nothing. Returns the exit code.
///
/// It loads and validates settings.json as `install` does and prints the same error, so a
/// caller can stop before it changes anything else.
pub fn check(cst: &Path) -> i32 {
    let Some(cst) = cst.to_str() else {
        let _ = writeln!(
            io::stderr().lock(),
            "error: {}: not a valid UTF-8 path",
            cst.display()
        );
        return 1;
    };
    check_at(&locations(), cst, &mut io::stderr().lock())
}

/// Takes `cst` out of Claude Code's settings.json. Returns the exit code.
pub fn uninstall() -> i32 {
    let (mut out, mut err) = (io::stdout().lock(), io::stderr().lock());
    uninstall_at(&locations(), &mut out, &mut err)
}

pub(crate) fn install_at(
    loc: &Locations,
    cst: &str,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    report(try_install(loc, cst, out), err)
}

pub(crate) fn check_at(loc: &Locations, cst: &str, err: &mut dyn Write) -> i32 {
    report(try_check(loc, cst), err)
}

pub(crate) fn uninstall_at(loc: &Locations, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    report(try_uninstall(loc, out), err)
}

fn report(result: Result<(), Failure>, err: &mut dyn Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(f) => {
            let _ = writeln!(
                err,
                "error: {}: {}; nothing was changed",
                f.path.display(),
                f.why
            );
            1
        }
    }
}

/// Returns the user's own status line saved by an earlier install, if any.
///
/// Without one of ours, the Python version's saved file is taken, but only while the status
/// line in settings.json is still ours: that is the case of an upgrade from it.
fn saved_status_line(loc: &Locations, settings: &Value) -> Result<Option<Value>, Failure> {
    match read_object(&loc.saved).map_err(Failure::io(&loc.settings))? {
        Read::Object(saved) => Ok(Some(saved)),
        Read::Invalid => Ok(None),
        Read::Missing => Ok(python_saved_status_line(loc, settings)),
    }
}

/// Returns the user's own status line saved by the Python version, if the status line in
/// `settings` is still ours and the file is a valid one.
fn python_saved_status_line(loc: &Locations, settings: &Value) -> Option<Value> {
    if !settings.get("statusLine").is_some_and(entry_is_ours) {
        return None;
    }
    match read_object(&loc.legacy_saved) {
        Ok(Read::Object(saved)) => Some(saved),
        _ => None,
    }
}

/// What `install` would do to the settings.
struct Plan {
    /// The settings after the change.
    new: Value,
    /// The user's own status line to save, if one is being replaced.
    to_save: Option<Value>,
    /// One line per change, as printed; empty when already installed.
    changes: Vec<String>,
}

/// Loads and validates the settings and works out what installing `cst` changes.
fn plan_install(loc: &Locations, cst: &str) -> Result<Plan, Failure> {
    let settings = load_settings(&loc.settings)?;
    let saved = saved_status_line(loc, &settings)?;
    let (new, to_save) = merge(&settings, cst, saved);
    let hooks_of = |s: &'_ Value, event: &str| s.get("hooks").and_then(|h| h.get(event)).cloned();
    let has_ours = |event: &str| {
        let groups = hooks_of(&settings, event);
        groups
            .and_then(|g| g.as_array().map(|g| g.iter().any(group_has_ours)))
            .unwrap_or(false)
    };
    let added = EVENTS.iter().filter(|e| !has_ours(e)).count();
    let updated = EVENTS
        .iter()
        .any(|e| has_ours(e) && hooks_of(&new, e) != hooks_of(&settings, e));
    let mut changes = Vec::new();
    if added > 0 {
        changes.push(format!("hooks: added to {added} events"));
    }
    if updated {
        changes.push(format!("hooks: updated to {cst}"));
    }
    let old_line = settings.get("statusLine");
    if new.get("statusLine") != old_line {
        changes.push(if to_save.is_some() {
            "status line: now cst statusline (yours is kept and still shown)".to_owned()
        } else if old_line.is_some() {
            format!("status line: updated to {cst}")
        } else {
            "status line: now cst statusline".to_owned()
        });
    }
    Ok(Plan {
        new,
        to_save,
        changes,
    })
}

/// Fails unless `dir`, or the nearest directory above it that exists, can be written to.
fn check_writable(dir: &Path, report_as: &Path) -> Result<(), Failure> {
    let existing = dir
        .ancestors()
        .find(|d| d.is_dir())
        .unwrap_or(Path::new("."));
    let wanted = rustix::fs::Access::WRITE_OK | rustix::fs::Access::EXEC_OK;
    rustix::fs::access(existing, wanted).map_err(|e| Failure::io(report_as)(e.into()))
}

fn try_check(loc: &Locations, cst: &str) -> Result<(), Failure> {
    let plan = plan_install(loc, cst)?;
    if plan.changes.is_empty() {
        return Ok(());
    }
    let real = real_path(&loc.settings);
    check_writable(real.parent().unwrap_or(Path::new(".")), &loc.settings)?;
    check_writable(&loc.state_dir, &loc.settings)
}

fn try_install(loc: &Locations, cst: &str, out: &mut dyn Write) -> Result<(), Failure> {
    let io_failure = Failure::io(&loc.settings);
    let Plan {
        new,
        to_save,
        mut changes,
    } = plan_install(loc, cst)?;
    if changes.is_empty() {
        let _ = writeln!(out, "already installed");
        return Ok(());
    }
    let real = real_path(&loc.settings);
    let dir = real.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(Failure::io(&loc.settings))?;
    fs::create_dir_all(&loc.state_dir).map_err(Failure::io(&loc.settings))?;
    if real.exists() {
        let backup = dir.join(BACKUP);
        // symlink_metadata: a dangling symlink in its place is not ours to replace
        if backup.symlink_metadata().is_err() {
            let bytes = fs::read(&real).map_err(Failure::io(&loc.settings))?;
            let mode = fs::metadata(&real)
                .map_err(Failure::io(&loc.settings))?
                .mode();
            paths::write_atomic(&backup, &bytes, Some(mode)).map_err(Failure::io(&loc.settings))?;
            changes.push(format!("backup: {}", backup.display()));
        }
    }
    if let Some(saved) = to_save {
        let bytes = serde_json::to_vec(&saved).map_err(io::Error::from);
        let bytes = bytes.map_err(&io_failure)?;
        paths::write_atomic(&loc.saved, &bytes, None).map_err(&io_failure)?;
    }
    let bytes = dump(&new).map_err(&io_failure)?;
    paths::write_atomic(&real, &bytes, None).map_err(&io_failure)?;
    let _ = writeln!(out, "{}", changes.join("\n"));
    Ok(())
}

fn try_uninstall(loc: &Locations, out: &mut dyn Write) -> Result<(), Failure> {
    let io_failure = Failure::io(&loc.settings);
    let settings = load_settings(&loc.settings)?;
    let saved = match read_object(&loc.saved).map_err(&io_failure)? {
        Read::Missing => python_saved_status_line(loc, &settings),
        Read::Invalid => return Err(Failure::new(&loc.saved, "not a valid JSON object")),
        Read::Object(saved) => Some(saved),
    };
    let had_saved = saved.is_some();
    let new = unmerge(&settings, saved);
    let mut changes = Vec::new();
    if new.get("hooks") != settings.get("hooks") {
        changes.push("hooks: removed");
    }
    if new.get("statusLine") != settings.get("statusLine") {
        changes.push(if had_saved {
            "status line: yours restored"
        } else {
            "status line: removed"
        });
    }
    if new != settings {
        let bytes = dump(&new).map_err(&io_failure)?;
        paths::write_atomic(&real_path(&loc.settings), &bytes, None).map_err(&io_failure)?;
    }
    match fs::remove_file(&loc.saved) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(io_failure(e)),
        _ => {}
    }
    if changes.is_empty() {
        let _ = writeln!(out, "nothing to uninstall");
    } else {
        let _ = writeln!(out, "{}", changes.join("\n"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::{Path, PathBuf};

    use serde_json::{json, Value};
    use tempfile::TempDir;

    use crate::claude::install::{
        check_at, install_at, is_ours, legacy_saved_path, merge, shape_error, split, uninstall_at,
        unmerge, Locations, BACKUP, EVENTS, HOOK_TIMEOUT,
    };

    const CS: &str = "/opt/bin/cs";

    fn foreign() -> Value {
        json!({"hooks": [{"type": "command", "command": "notify-me"}]})
    }

    fn sample() -> Value {
        json!({
            "model": "opus",
            "hooks": {"Stop": [foreign()], "Custom": [foreign()]},
            "statusLine": {"type": "command", "command": "mine.sh", "padding": 2},
        })
    }

    fn keys(value: &Value) -> Vec<&str> {
        value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    /// The command of our entries in an event's groups.
    fn ours_in(settings: &Value, event: &str) -> Vec<String> {
        settings["hooks"][event]
            .as_array()
            .expect("groups")
            .iter()
            .flat_map(|g| g["hooks"].as_array().expect("hooks").iter())
            .filter_map(|h| h["command"].as_str())
            .filter(|c| is_ours(c))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn merge_keeps_foreign_hooks_on_same_event() {
        let (new, _) = merge(&sample(), CS, None);
        assert_eq!(new["hooks"]["Stop"][0], foreign());
        assert_eq!(
            new["hooks"]["Stop"][1]["hooks"][0]["command"],
            "/opt/bin/cs hook"
        );
        assert_eq!(new["hooks"]["Stop"][1]["hooks"][0]["timeout"], 10);
        assert_eq!(EVENTS.len(), 11);
        for event in EVENTS {
            assert!(new["hooks"].get(event).is_some(), "{event}");
        }
    }

    #[test]
    fn the_hook_timeout_is_ten_seconds() {
        assert_eq!(HOOK_TIMEOUT, 10);
    }

    #[test]
    fn merge_is_idempotent() {
        let (once, saved) = merge(&sample(), CS, None);
        let (twice, saved2) = merge(&once, CS, saved.clone());
        assert_eq!(twice, once);
        assert_eq!(saved2, saved);
    }

    #[test]
    fn merge_does_not_modify_its_input() {
        let before = sample();
        let _ = merge(&before, CS, None);
        assert_eq!(before, sample());
    }

    #[test]
    fn merge_wraps_user_statusline_keeping_padding() {
        let (new, saved) = merge(&sample(), CS, None);
        assert_eq!(
            new["statusLine"],
            json!({"type": "command", "command": "/opt/bin/cs statusline", "padding": 2})
        );
        assert_eq!(
            saved,
            Some(json!({"type": "command", "command": "mine.sh", "padding": 2}))
        );
    }

    #[test]
    fn merge_creates_statusline_when_absent() {
        let (new, saved) = merge(&json!({}), CS, None);
        assert_eq!(
            new["statusLine"],
            json!({"type": "command", "command": "/opt/bin/cs statusline"})
        );
        assert_eq!(saved, None);
    }

    #[test]
    fn unmerge_restores_original_exactly() {
        let original = sample();
        let (new, saved) = merge(&original, CS, None);
        assert_eq!(unmerge(&new, saved), original);
    }

    #[test]
    fn unmerge_without_saved_removes_our_statusline() {
        let (new, saved) = merge(&json!({"model": "opus"}), CS, None);
        assert_eq!(unmerge(&new, saved), json!({"model": "opus"}));
    }

    #[test]
    fn a_cs_path_with_a_space_is_quoted_and_still_ours() {
        let cs = "/opt/my tools/cs";
        let (new, _) = merge(&json!({}), cs, None);
        let command = new["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command");
        assert_eq!(command, "'/opt/my tools/cs' hook");
        assert!(is_ours(command));
        assert_eq!(
            new["statusLine"]["command"],
            "'/opt/my tools/cs' statusline"
        );
        assert_eq!(unmerge(&new, None), json!({}));
    }

    #[test]
    fn is_ours_accepts_cs_and_cst_by_any_path() {
        assert!(is_ours("\"/x/cs\" hook"));
        assert!(is_ours("/x/cs statusline"));
        assert!(is_ours("/home/u/.local/bin/cst hook"));
        assert!(is_ours("cst statusline"));
        assert!(is_ours("'/opt/my tools/cst' hook"));
    }

    #[test]
    fn is_ours_rejects_everything_else() {
        assert!(!is_ours("\"/x/cs\" panel"));
        assert!(!is_ours("/x/other hook"));
        assert!(!is_ours("/x/csx hook"));
        assert!(!is_ours("/x/cst hook extra"));
        assert!(!is_ours("/x/cst"));
        assert!(!is_ours("\"unterminated hook"));
        assert!(!is_ours("trailing\\"));
        assert!(!is_ours(""));
    }

    #[test]
    fn split_follows_the_shell_rules() {
        let cases: [(&str, Option<Vec<&str>>); 8] = [
            ("a b", Some(vec!["a", "b"])),
            ("  a \t b\n", Some(vec!["a", "b"])),
            ("'a b' c", Some(vec!["a b", "c"])),
            (r#""a \"b\" \\ \n" c"#, Some(vec![r#"a "b" \ \n"#, "c"])),
            (r"a\ b c", Some(vec!["a b", "c"])),
            ("'' x", Some(vec!["", "x"])),
            ("'open", None),
            (r"end\", None),
        ];
        for (input, want) in cases {
            let want = want.map(|v| v.into_iter().map(str::to_owned).collect::<Vec<_>>());
            assert_eq!(split(input), want, "{input:?}");
        }
    }

    #[test]
    fn shape_error_names_what_is_malformed() {
        let cases = [
            (json!({"hooks": []}), "hooks is not an object"),
            (json!({"hooks": null}), "hooks is not an object"),
            (json!({"hooks": {"Stop": {}}}), "hooks.Stop is not a list"),
            (
                json!({"hooks": {"Stop": [{}]}}),
                "hooks.Stop has a group without a hooks list",
            ),
            (
                json!({"hooks": {"Stop": [{"hooks": {}}]}}),
                "hooks.Stop has a group without a hooks list",
            ),
            (json!({"statusLine": "x"}), "statusLine is not an object"),
            (json!({"statusLine": null}), "statusLine is not an object"),
        ];
        for (settings, why) in cases {
            assert_eq!(shape_error(&settings).as_deref(), Some(why), "{settings}");
        }
        assert_eq!(shape_error(&sample()), None);
        assert_eq!(shape_error(&json!({})), None);
    }

    #[test]
    fn merge_repoints_stale_path() {
        let stale = json!({"hooks": [{"type": "command", "command": "\"/old/cs\" hook"}]});
        let hooks: serde_json::Map<String, Value> = EVENTS[..4]
            .iter()
            .map(|e| ((*e).to_owned(), json!([foreign(), stale])))
            .collect();
        let old = json!({
            "hooks": hooks,
            "statusLine": {"type": "command", "command": "\"/old/cs\" statusline", "padding": 1},
        });
        let saved = json!({"type": "command", "command": "mine.sh"});
        let (new, kept) = merge(&old, CS, Some(saved.clone()));
        for event in EVENTS {
            assert_eq!(ours_in(&new, event), vec!["/opt/bin/cs hook"], "{event}");
        }
        assert_eq!(
            new["statusLine"],
            json!({"type": "command", "command": "/opt/bin/cs statusline", "padding": 1})
        );
        assert_eq!(kept, Some(saved));
    }

    #[test]
    fn mixed_foreign_and_ours_group_round_trips() {
        let original = json!({"hooks": {"Stop": [{"hooks": [
            foreign()["hooks"][0].clone(),
            {"type": "command", "command": "\"/x/cs\" hook"},
        ]}]}});
        let (new, saved) = merge(&original, CS, None);
        assert_eq!(new["hooks"]["Stop"].as_array().expect("groups").len(), 1);
        let out = unmerge(&new, saved);
        assert_eq!(
            out["hooks"]["Stop"],
            json!([{"hooks": [foreign()["hooks"][0]]}])
        );
    }

    #[test]
    fn unmerge_drops_only_the_containers_it_emptied() {
        let ours = json!({"hooks": [{"type": "command", "command": "/opt/bin/cs hook"}]});
        assert_eq!(
            unmerge(&json!({"hooks": {"Stop": [ours]}}), None),
            json!({})
        );
        assert_eq!(
            unmerge(&json!({"hooks": {"Stop": [ours], "X": []}}), None),
            json!({"hooks": {"X": []}})
        );
        // Nothing of ours was in it: an empty container is not ours to drop
        let line = json!({"type": "command", "command": "/opt/bin/cs statusline"});
        assert_eq!(
            unmerge(&json!({"hooks": {}, "statusLine": line}), None),
            json!({"hooks": {}})
        );
    }

    #[test]
    fn unmerge_keeps_a_group_matcher_and_the_other_groups() {
        let group = json!({"matcher": "Bash", "hooks": [
            {"type": "command", "command": "/opt/bin/cst hook"},
            {"type": "command", "command": "mine"},
        ]});
        let out = unmerge(&json!({"hooks": {"PreToolUse": [group, foreign()]}}), None);
        assert_eq!(
            out["hooks"]["PreToolUse"],
            json!([
                {"matcher": "Bash", "hooks": [{"type": "command", "command": "mine"}]},
                foreign()
            ])
        );
    }

    /// A temp dir with the places `install` reads and writes, and a way to run it.
    struct Rig {
        dir: TempDir,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                dir: TempDir::new().expect("temp dir"),
            }
        }

        fn settings(&self) -> PathBuf {
            self.dir.path().join("claude/settings.json")
        }

        fn saved(&self) -> PathBuf {
            self.dir.path().join("state/statusline.json")
        }

        fn legacy(&self) -> PathBuf {
            self.dir.path().join("legacy/statusline.json")
        }

        fn loc(&self) -> Locations {
            Locations {
                settings: self.settings(),
                state_dir: self.dir.path().join("state"),
                saved: self.saved(),
                legacy_saved: self.legacy(),
            }
        }

        fn put(&self, path: &Path, text: &str) {
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(path, text).expect("write");
        }

        fn put_settings(&self, value: &Value) {
            let text = serde_json::to_string_pretty(value).expect("json");
            self.put(&self.settings(), &text);
        }

        fn install(&self, cst: &str) -> (i32, String, String) {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = install_at(&self.loc(), cst, &mut out, &mut err);
            (code, text(out), text(err))
        }

        fn uninstall(&self) -> (i32, String, String) {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = uninstall_at(&self.loc(), &mut out, &mut err);
            (code, text(out), text(err))
        }

        fn read(&self, path: &Path) -> Value {
            serde_json::from_slice(&fs::read(path).expect("read")).expect("json")
        }
    }

    fn text(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).expect("utf-8")
    }

    /// Every file and directory under `root`, with its size, to tell that nothing was written.
    fn tree(root: &Path) -> Vec<(PathBuf, u64)> {
        let mut found = Vec::new();
        let mut todo = vec![root.to_path_buf()];
        while let Some(dir) = todo.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.map(|e| e.expect("entry")) {
                let meta = entry.metadata().expect("meta");
                if meta.is_dir() {
                    todo.push(entry.path());
                }
                found.push((entry.path(), meta.len()));
            }
        }
        found.sort();
        found
    }

    impl Rig {
        fn check(&self) -> (i32, String) {
            let mut err = Vec::new();
            let code = check_at(&self.loc(), CS, &mut err);
            (code, text(err))
        }
    }

    #[test]
    fn check_accepts_valid_settings_and_writes_nothing() {
        let rig = Rig::new();
        rig.put_settings(&sample());
        let before = tree(rig.dir.path());
        assert_eq!(rig.check(), (0, String::new()));
        assert_eq!(tree(rig.dir.path()), before);
    }

    #[test]
    fn check_accepts_missing_settings_and_creates_no_directory() {
        let rig = Rig::new();
        assert_eq!(rig.check(), (0, String::new()));
        assert!(tree(rig.dir.path()).is_empty());
    }

    #[test]
    fn check_refuses_malformed_settings_with_the_error_install_gives() {
        let cases = ["{oops", "[1]", r#"{"hooks": []}"#, r#"{"statusLine": "x"}"#];
        for body in cases {
            let rig = Rig::new();
            rig.put(&rig.settings(), body);
            let before = tree(rig.dir.path());
            let (_, _, install_err) = rig.install(CS);
            // The failed install wrote nothing either
            assert_eq!(tree(rig.dir.path()), before, "{body}");
            let (code, err) = rig.check();
            assert_eq!((code, err.as_str()), (1, install_err.as_str()), "{body}");
            assert_eq!(tree(rig.dir.path()), before, "{body}");
        }
    }

    #[test]
    fn check_refuses_a_settings_directory_it_cannot_write() {
        let rig = Rig::new();
        rig.put_settings(&sample());
        let dir = rig.settings().parent().expect("parent").to_path_buf();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).expect("chmod");
        let (_, _, install_err) = rig.install(CS);
        let (code, err) = rig.check();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).expect("chmod");
        assert_eq!(code, 1, "{err}");
        assert!(err.starts_with("error: "), "{err}");
        assert!(err.ends_with("nothing was changed\n"), "{err}");
        assert_eq!(err.split(": ").nth(2), install_err.split(": ").nth(2));
    }

    #[test]
    fn check_accepts_an_installed_settings_in_a_directory_it_cannot_write() {
        let rig = Rig::new();
        assert_eq!(rig.install(CS).0, 0);
        let dir = rig.settings().parent().expect("parent").to_path_buf();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).expect("chmod");
        let result = rig.check();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).expect("chmod");
        // Install would only say "already installed": nothing to write
        assert_eq!(result, (0, String::new()));
    }

    #[test]
    fn check_refuses_a_state_directory_it_cannot_create() {
        let rig = Rig::new();
        rig.put_settings(&sample());
        let blocked = rig.dir.path().to_path_buf();
        // The state directory would go in the temp dir itself
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o500)).expect("chmod");
        let (code, err) = rig.check();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).expect("chmod");
        assert_eq!(code, 1, "{err}");
        assert!(err.ends_with("nothing was changed\n"), "{err}");
    }

    #[test]
    fn install_follows_a_symlinked_settings_file() {
        let rig = Rig::new();
        let real = rig.dir.path().join("dots/settings.json");
        rig.put(&real, &sample().to_string());
        fs::create_dir_all(rig.settings().parent().expect("parent")).expect("mkdir");
        symlink(&real, rig.settings()).expect("symlink");
        assert_eq!(rig.install(CS).0, 0);
        assert!(fs::symlink_metadata(rig.settings())
            .expect("meta")
            .is_symlink());
        assert!(rig.read(&real).get("hooks").is_some());
        assert!(fs::read_to_string(&real).expect("read").ends_with("}\n"));
        assert!(real.parent().expect("parent").join(BACKUP).exists());
        assert_eq!(rig.read(&rig.saved()), sample()["statusLine"]);
        let (code, out, _) = rig.install(CS);
        assert_eq!((code, out.as_str()), (0, "already installed\n"));
        assert_eq!(rig.uninstall().0, 0);
        assert_eq!(rig.read(&real), sample());
        assert!(fs::symlink_metadata(rig.settings())
            .expect("meta")
            .is_symlink());
        assert!(!rig.saved().exists());
    }

    #[test]
    fn install_follows_a_dangling_symlink_and_creates_its_target() {
        let rig = Rig::new();
        let real = rig.dir.path().join("dots/settings.json");
        fs::create_dir_all(real.parent().expect("parent")).expect("mkdir");
        fs::create_dir_all(rig.settings().parent().expect("parent")).expect("mkdir");
        symlink(&real, rig.settings()).expect("symlink");
        assert_eq!(rig.install(CS).0, 0);
        assert!(fs::symlink_metadata(rig.settings())
            .expect("meta")
            .is_symlink());
        assert!(rig.read(&real).get("hooks").is_some());
    }

    #[test]
    fn install_on_missing_settings_creates_the_file() {
        let rig = Rig::new();
        let (code, out, _) = rig.install(CS);
        assert_eq!(code, 0);
        assert_eq!(
            out,
            "hooks: added to 11 events\nstatus line: now cst statusline\n"
        );
        let data = rig.read(&rig.settings());
        assert_eq!(keys(&data["hooks"]), EVENTS);
        assert!(!rig.saved().exists());
        assert!(!rig.settings().with_file_name(BACKUP).exists());
        assert_eq!(rig.uninstall().0, 0);
        assert_eq!(rig.read(&rig.settings()), json!({}));
    }

    #[test]
    fn install_refuses_invalid_json_and_changes_nothing() {
        let rig = Rig::new();
        rig.put(&rig.settings(), "{oops");
        let (code, out, err) = rig.install(CS);
        assert_eq!(code, 1);
        assert_eq!(out, "");
        assert!(err.starts_with("error: "), "{err}");
        assert!(err.contains("settings.json"), "{err}");
        assert!(
            err.ends_with("not a valid JSON object; nothing was changed\n"),
            "{err}"
        );
        assert_eq!(fs::read_to_string(rig.settings()).expect("read"), "{oops");
        assert!(!rig.saved().exists());
    }

    #[test]
    fn a_top_level_that_is_not_an_object_is_refused() {
        let rig = Rig::new();
        rig.put(&rig.settings(), "[1]");
        assert_eq!(rig.install(CS).0, 1);
        assert_eq!(rig.uninstall().0, 1);
        assert_eq!(fs::read_to_string(rig.settings()).expect("read"), "[1]");
    }

    #[test]
    fn install_reports_an_updated_path() {
        let rig = Rig::new();
        rig.install("/old/cs");
        let (code, out, _) = rig.install(CS);
        assert_eq!(code, 0);
        assert!(out.contains("hooks: updated to /opt/bin/cs"), "{out}");
        assert!(out.contains("status line: updated to /opt/bin/cs"), "{out}");
    }

    #[test]
    fn install_reports_a_kept_status_line_and_the_backup() {
        let rig = Rig::new();
        rig.put_settings(&sample());
        let (_, out, _) = rig.install(CS);
        let backup = rig.settings().with_file_name(BACKUP);
        assert_eq!(
            out,
            format!(
                "hooks: added to 11 events\n\
                 status line: now cst statusline (yours is kept and still shown)\n\
                 backup: {}\n",
                backup.display()
            )
        );
    }

    #[test]
    fn the_backup_is_made_once() {
        let rig = Rig::new();
        let original = serde_json::to_string_pretty(&sample()).expect("json");
        rig.put(&rig.settings(), &original);
        rig.install("/old/cs");
        rig.install(CS);
        let backup = rig.settings().with_file_name(BACKUP);
        assert_eq!(fs::read_to_string(backup).expect("read"), original);
    }

    #[test]
    fn reinstall_with_saved_and_user_statusline() {
        let rig = Rig::new();
        rig.put_settings(&json!({"statusLine": {"type": "command", "command": "first.sh"}}));
        rig.install(CS);
        let mut data = rig.read(&rig.settings());
        data["statusLine"] = json!({"type": "command", "command": "second.sh"});
        rig.put_settings(&data);
        rig.install(CS);
        assert_eq!(rig.read(&rig.saved())["command"], "second.sh");
        rig.uninstall();
        assert_eq!(
            rig.read(&rig.settings())["statusLine"]["command"],
            "second.sh"
        );
    }

    #[test]
    fn statusline_already_ours_before_first_install() {
        let rig = Rig::new();
        let line = json!({"type": "command", "command": "/opt/bin/cs statusline"});
        rig.put_settings(&json!({"statusLine": line}));
        rig.install(CS);
        assert_eq!(rig.read(&rig.settings())["statusLine"], line);
        assert!(!rig.saved().exists());
        rig.uninstall();
        assert!(rig.read(&rig.settings()).get("statusLine").is_none());
    }

    #[test]
    fn uninstall_says_restored_for_an_empty_saved_line() {
        let rig = Rig::new();
        rig.put(&rig.settings(), r#"{"statusLine": {}}"#);
        rig.install(CS);
        let (code, out, _) = rig.uninstall();
        assert_eq!(code, 0);
        assert!(out.contains("status line: yours restored"), "{out}");
        assert_eq!(rig.read(&rig.settings()), json!({"statusLine": {}}));
    }

    #[test]
    fn uninstall_with_nothing_of_ours_says_so() {
        let rig = Rig::new();
        rig.put_settings(&sample());
        let before = fs::read(rig.settings()).expect("read");
        let (code, out, _) = rig.uninstall();
        assert_eq!((code, out.as_str()), (0, "nothing to uninstall\n"));
        assert_eq!(fs::read(rig.settings()).expect("read"), before);
    }

    #[test]
    fn malformed_shapes_are_refused() {
        let rig = Rig::new();
        for bad in [
            r#"{"hooks": []}"#,
            r#"{"hooks": {"Stop": {}}}"#,
            r#"{"hooks": {"Stop": [{}]}}"#,
        ] {
            rig.put(&rig.settings(), bad);
            assert_eq!(rig.install(CS).0, 1, "{bad}");
            let (code, _, err) = rig.uninstall();
            assert_eq!(code, 1, "{bad}");
            assert!(err.contains("settings.json"), "{err}");
            assert_eq!(fs::read_to_string(rig.settings()).expect("read"), bad);
        }
    }

    #[test]
    fn unreadable_settings_fail_cleanly() {
        let rig = Rig::new();
        // A directory where the file should be
        fs::create_dir_all(rig.settings()).expect("mkdir");
        let (code, _, err) = rig.install(CS);
        assert_eq!(code, 1);
        assert!(err.starts_with("error: "), "{err}");
        assert!(err.contains("Is a directory; nothing was changed"), "{err}");
    }

    #[test]
    fn install_keeps_the_file_mode_and_leaves_no_temp() {
        let rig = Rig::new();
        rig.put(&rig.settings(), r#"{"note": "é"}"#);
        fs::set_permissions(rig.settings(), fs::Permissions::from_mode(0o640)).expect("chmod");
        rig.install(CS);
        let mode = |p: &Path| fs::metadata(p).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode(&rig.settings()), 0o640);
        assert_eq!(mode(&rig.settings().with_file_name(BACKUP)), 0o640);
        assert!(fs::read_to_string(rig.settings())
            .expect("read")
            .contains('é'));
        let temps = fs::read_dir(rig.settings().parent().expect("parent"))
            .expect("dir")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
            .count();
        assert_eq!(temps, 0);
    }

    #[test]
    fn uninstall_refuses_a_broken_saved_statusline() {
        let rig = Rig::new();
        rig.put_settings(&sample());
        rig.install(CS);
        let installed = fs::read(rig.settings()).expect("read");
        fs::write(rig.saved(), "{not json").expect("write");
        let (code, _, err) = rig.uninstall();
        assert_eq!(code, 1);
        assert!(err.contains("statusline.json"), "{err}");
        assert_eq!(fs::read(rig.settings()).expect("read"), installed);
        assert_eq!(fs::read_to_string(rig.saved()).expect("read"), "{not json");
    }

    #[test]
    fn a_broken_saved_statusline_does_not_stop_a_reinstall() {
        let rig = Rig::new();
        rig.put_settings(&sample());
        rig.install(CS);
        fs::write(rig.saved(), "{not json").expect("write");
        assert_eq!(rig.install("/new/cs").0, 0);
        assert_eq!(fs::read_to_string(rig.saved()).expect("read"), "{not json");
    }

    fn python_install(rig: &Rig) {
        let hooks: serde_json::Map<String, Value> = EVENTS
            .iter()
            .map(|e| {
                let entry = json!({"type": "command", "command": "/home/u/.local/bin/cs hook"});
                ((*e).to_owned(), json!([{"hooks": [entry]}]))
            })
            .collect();
        rig.put_settings(&json!({
            "hooks": hooks,
            "statusLine": {"type": "command", "command": "/home/u/.local/bin/cs statusline"},
        }));
    }

    #[test]
    fn uninstall_over_a_python_install_restores_its_saved_status_line() {
        let rig = Rig::new();
        python_install(&rig);
        let mine = json!({"type": "command", "command": "~/.claude/statusline.sh"});
        rig.put(&rig.legacy(), &mine.to_string());
        let (code, out, _) = rig.uninstall();
        assert_eq!(code, 0);
        assert!(out.contains("status line: yours restored"), "{out}");
        assert_eq!(rig.read(&rig.settings())["statusLine"], mine);
        // The Python version's file is not ours to delete
        assert!(rig.legacy().exists());
    }

    #[test]
    fn uninstall_ignores_the_python_saved_line_when_the_current_one_is_not_ours() {
        let rig = Rig::new();
        rig.put_settings(&sample());
        rig.put(&rig.legacy(), r#"{"type": "command", "command": "old.sh"}"#);
        rig.uninstall();
        assert_eq!(
            rig.read(&rig.settings())["statusLine"],
            sample()["statusLine"]
        );
    }

    #[test]
    fn uninstall_still_refuses_a_broken_saved_line_despite_a_python_one() {
        let rig = Rig::new();
        python_install(&rig);
        rig.put(&rig.saved(), "{not json");
        rig.put(&rig.legacy(), r#"{"type": "command", "command": "old.sh"}"#);
        let before = fs::read(rig.settings()).expect("read");
        assert_eq!(rig.uninstall().0, 1);
        assert_eq!(fs::read(rig.settings()).expect("read"), before);
    }

    #[test]
    fn the_python_saved_status_line_is_carried_over() {
        let rig = Rig::new();
        python_install(&rig);
        let mine = json!({"type": "command", "command": "~/.claude/statusline.sh"});
        rig.put(&rig.legacy(), &mine.to_string());
        let (code, out, _) = rig.install(CS);
        assert_eq!(code, 0);
        assert!(out.contains("hooks: updated to /opt/bin/cs"), "{out}");
        assert_eq!(rig.read(&rig.saved()), mine);
        assert_eq!(rig.uninstall().0, 0);
        assert_eq!(rig.read(&rig.settings()), json!({"statusLine": mine}));
    }

    #[test]
    fn the_python_saved_status_line_is_ignored_when_the_current_one_is_not_ours() {
        let rig = Rig::new();
        rig.put_settings(&sample());
        rig.put(&rig.legacy(), r#"{"type": "command", "command": "old.sh"}"#);
        rig.install(CS);
        assert_eq!(rig.read(&rig.saved())["command"], "mine.sh");
    }

    #[test]
    fn the_cst_saved_status_line_wins_over_the_python_one() {
        let rig = Rig::new();
        python_install(&rig);
        rig.put(&rig.saved(), r#"{"type": "command", "command": "new.sh"}"#);
        rig.put(&rig.legacy(), r#"{"type": "command", "command": "old.sh"}"#);
        rig.install(CS);
        assert_eq!(rig.read(&rig.saved())["command"], "new.sh");
    }

    #[test]
    fn a_broken_python_saved_status_line_is_ignored() {
        let rig = Rig::new();
        python_install(&rig);
        rig.put(&rig.legacy(), "{nope");
        assert_eq!(rig.install(CS).0, 0);
        assert!(!rig.saved().exists());
    }

    #[test]
    fn migration_leaves_no_duplicate_groups() {
        let rig = Rig::new();
        python_install(&rig);
        rig.install(CS);
        let data = rig.read(&rig.settings());
        for event in EVENTS {
            let groups = data["hooks"][event].as_array().expect("groups");
            assert_eq!(groups.len(), 1, "{event}");
            assert_eq!(ours_in(&data, event), vec!["/opt/bin/cs hook"]);
        }
        assert_eq!(data["statusLine"]["command"], "/opt/bin/cs statusline");
    }

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn the_python_state_dir_follows_its_variables() {
        let home = Path::new("/home/u");
        let path = |pairs: &[(&str, &str)]| legacy_saved_path(&env_of(pairs), home);
        assert_eq!(
            path(&[("CLAUDE_SESSIONS_STATE", "/s"), ("XDG_STATE_HOME", "/x")]),
            Path::new("/s/statusline.json")
        );
        assert_eq!(
            path(&[("XDG_STATE_HOME", "/x")]),
            Path::new("/x/claude-sessions/statusline.json")
        );
        assert_eq!(
            path(&[("CLAUDE_SESSIONS_STATE", ""), ("XDG_STATE_HOME", "")]),
            Path::new("/home/u/.local/state/claude-sessions/statusline.json")
        );
    }
}
