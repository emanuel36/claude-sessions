//! Session records on disk: one JSON file per live Claude Code session.
//!
//! Records are `<id>.json` in the runtime directory, each with a lock file `<id>.json.lock`.
//! Every public function is a thin wrapper over one that takes the directory and a journal
//! sink, so unit tests use a temporary directory and never touch the process environment or
//! the real journal.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fs::{flock, FlockOperation};

use crate::core::journal::{self, JournalEvent};
use crate::core::paths::{self, LIMITS_FILE};
use crate::core::state::{self, Record};

/// A lock file nobody holds, whose session has no record, is deleted once this old.
pub const STALE_LOCK_SECONDS: f64 = 60.0;

const RECORD_SUFFIX: &str = ".json";

/// Where the store reports status changes; the journal outside of tests.
pub(crate) type Sink<'a> = &'a dyn Fn(&JournalEvent);

/// An exclusive lock on one session's record, released when dropped.
#[derive(Debug)]
pub struct Lock {
    _file: File,
}

/// Tells whether a value can name a session record: letters, digits, `-` and `_`.
pub fn is_session_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// Takes the lock of a session, waiting for whoever holds it.
///
/// Hooks of parallel tool calls can overlap, so a read-modify-write of a record goes under it.
///
/// # Errors
///
/// Fails if the id is not a [`is_session_id`], or the lock file cannot be opened or locked.
pub fn lock(id: &str) -> io::Result<Lock> {
    lock_in(&paths::runtime_dir()?, id)
}

/// Returns the record of a session, or `None` if there is none or it is not readable.
pub fn read(id: &str) -> Option<Record> {
    read_in(&paths::runtime_dir().ok()?, id)
}

/// Writes a record atomically, so readers never see a partial file.
///
/// A change of status, or a new session, also goes to the journal, at [`state::since`]. Callers
/// hold the lock, so the record on disk is the one this write replaces.
///
/// # Errors
///
/// Fails if the id is not a [`is_session_id`] or the file cannot be written.
pub fn write(r: &Record) -> io::Result<()> {
    write_in(&paths::runtime_dir()?, r, &journal::append)
}

/// Deletes a session's record and, unless a hook may still hold it, its lock.
///
/// Removing a record ends the session in the journal, so a wait it was in stops counting.
/// Failures go to the error log.
pub fn remove(id: &str, keep_lock: bool) {
    match paths::runtime_dir() {
        Ok(dir) => remove_in(&dir, id, keep_lock, &journal::append),
        Err(e) => paths::log_error(&format_args!("store: cannot remove {id}: {e}")),
    }
}

/// Deletes the records of sessions this one replaced in the same Claude process.
///
/// `/clear` and `/resume` start another session in the same process; the one before it ends
/// without a `SessionEnd`, and would otherwise linger until the process exits. The lock stays,
/// as on `SessionEnd`: a hook of the old session may be waiting on it.
pub fn remove_replaced(r: &Record) {
    match paths::runtime_dir() {
        Ok(dir) => remove_replaced_in(&dir, r, &journal::append),
        Err(e) => paths::log_error(&format_args!("store: cannot remove replaced: {e}")),
    }
}

/// Returns the start time of a process from `/proc`, or `None` if it is gone.
pub fn process_start(pid: i32) -> Option<u64> {
    let stat = fs::read(format!("/proc/{pid}/stat")).ok()?;
    parse_start(&stat)
}

/// Tells whether the Claude process behind a record still runs, and is the same one.
pub fn is_alive(r: &Record) -> bool {
    let Some(pid) = r.pid else {
        return false;
    };
    process_start(pid) == r.pid_start
}

/// Returns the records of all live sessions, ordered for display.
///
/// Records of dead processes are deleted, and so are the locks ended sessions left behind.
/// `now` is the current time as a Unix timestamp.
pub fn load_all(now: f64) -> Vec<Record> {
    match paths::runtime_dir() {
        Ok(dir) => load_all_in(&dir, now, &journal::append),
        Err(e) => {
            paths::log_error(&format_args!("store: cannot list records: {e}"));
            Vec::new()
        }
    }
}

/// Reads field 22 (`starttime`) of the contents of `/proc/<pid>/stat`.
///
/// The second field, the command name, is in parentheses and may hold spaces and parentheses
/// itself, so the fields are counted from the last `)`.
fn parse_start(stat: &[u8]) -> Option<u64> {
    let close = stat.iter().rposition(|&b| b == b')')?;
    let rest = std::str::from_utf8(&stat[close + 1..]).ok()?;
    // `rest` starts at field 3, so field 22 is the 20th
    rest.split_ascii_whitespace().nth(19)?.parse().ok()
}

/// The session id of a record's file name; `None` for any other file.
fn record_id(name: Option<&str>) -> Option<&str> {
    let name = name.filter(|&n| n != LIMITS_FILE)?;
    name.strip_suffix(RECORD_SUFFIX)
        .filter(|id| is_session_id(id))
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Removes the lock an ended session left behind, once no hook can still be waiting on it.
fn drop_stale_lock(path: &Path, now: f64) {
    let age = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|t| now - t.as_secs_f64());
    if age.is_some_and(|age| age > STALE_LOCK_SECONDS) {
        // Gone already, or not ours to remove: either way the next sweep tries again
        let _ = fs::remove_file(path);
    }
}

fn record_path(dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_session_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("bad session id {id:?}"),
        ));
    }
    Ok(dir.join(format!("{id}{RECORD_SUFFIX}")))
}

pub(crate) fn lock_in(dir: &Path, id: &str) -> io::Result<Lock> {
    let mut path = record_path(dir, id)?.into_os_string();
    path.push(".lock");
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)?;
    loop {
        match flock(&file, FlockOperation::LockExclusive) {
            Ok(()) => return Ok(Lock { _file: file }),
            Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
    }
}

pub(crate) fn read_in(dir: &Path, id: &str) -> Option<Record> {
    let bytes = fs::read(record_path(dir, id).ok()?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn write_in(dir: &Path, r: &Record, journal: Sink<'_>) -> io::Result<()> {
    let path = record_path(dir, &r.id)?;
    let before = read_in(dir, &r.id);
    let data = serde_json::to_vec(r).map_err(io::Error::other)?;
    paths::write_atomic(&path, &data, None)?;
    if before.is_none_or(|b| b.status != r.status) {
        let since = state::since(r);
        journal(&JournalEvent {
            t: if since != 0.0 { since } else { unix_now() },
            id: r.id.clone(),
            status: Some(r.status.as_str().to_owned()),
            ..JournalEvent::default()
        });
    }
    Ok(())
}

pub(crate) fn remove_in(dir: &Path, id: &str, keep_lock: bool, journal: Sink<'_>) {
    let Ok(path) = record_path(dir, id) else {
        return;
    };
    match fs::remove_file(&path) {
        Ok(()) => journal(&JournalEvent {
            t: unix_now(),
            id: id.to_owned(),
            status: Some(journal::ENDED.to_owned()),
            ..JournalEvent::default()
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => paths::log_error(&format_args!("store: cannot remove {id}: {e}")),
    }
    if !keep_lock {
        let mut lock = path.into_os_string();
        lock.push(".lock");
        match fs::remove_file(&lock) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                paths::log_error(&format_args!("store: cannot remove lock of {id}: {e}"));
            }
            _ => {}
        }
    }
}

pub(crate) fn remove_replaced_in(dir: &Path, r: &Record, journal: Sink<'_>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(id) = record_id(name.to_str()) else {
            continue;
        };
        if id == r.id {
            continue;
        }
        // A session that just ended is no reason to fail this one
        let Ok(_lock) = lock_in(dir, id) else {
            continue;
        };
        if let Some(other) = read_in(dir, id) {
            if (other.pid, other.pid_start) == (r.pid, r.pid_start) {
                remove_in(dir, id, true, journal);
            }
        }
    }
}

pub(crate) fn load_all_in(dir: &Path, now: f64, journal: Sink<'_>) -> Vec<Record> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let names: HashSet<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    let mut records = Vec::new();
    for name in &names {
        if let Some(record) = name.strip_suffix(".lock") {
            if !names.contains(record) {
                drop_stale_lock(&dir.join(name), now);
            }
        }
        let Some(id) = record_id(Some(name)) else {
            continue;
        };
        let Some(record) = read_in(dir, id) else {
            continue;
        };
        if is_alive(&record) {
            records.push(record);
        } else {
            // A process that dies without SessionEnd (a closed window, a crash) still ends
            remove_in(dir, id, false, journal);
        }
    }
    records.sort_by(|a, b| {
        let (a, b) = (state::sort_key(a), state::sort_key(b));
        a.0.cmp(&b.0).then(a.1.total_cmp(&b.1))
    });
    records
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    use tempfile::TempDir;

    use crate::core::journal::{JournalEvent, ENDED};
    use crate::core::paths::LIMITS_FILE;
    use crate::core::state::{new_record, Pending, Record, Status, Waiting};
    use crate::core::store::{
        is_alive, is_session_id, load_all_in, lock_in, parse_start, process_start, read_in,
        remove_in, remove_replaced_in, write_in, STALE_LOCK_SECONDS,
    };

    fn quiet(_: &JournalEvent) {}

    fn live_record(id: &str) -> Record {
        let pid = std::process::id() as i32;
        Record {
            pid: Some(pid),
            pid_start: process_start(pid),
            ..new_record(id)
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn ids(records: &[Record]) -> Vec<&str> {
        records.iter().map(|r| r.id.as_str()).collect()
    }

    /// Runs `f` with a sink that collects the journal events it receives.
    fn journaled(f: impl FnOnce(&dyn Fn(&JournalEvent))) -> Vec<JournalEvent> {
        let events = RefCell::new(Vec::new());
        f(&|e| events.borrow_mut().push(e.clone()));
        events.into_inner()
    }

    #[test]
    fn write_then_read_round_trips() {
        let tmp = TempDir::new().unwrap();
        let r = Record {
            status: Status::Done,
            ..live_record("s1")
        };
        write_in(tmp.path(), &r, &quiet).unwrap();
        assert_eq!(read_in(tmp.path(), "s1"), Some(r));
    }

    #[test]
    fn reading_a_missing_or_broken_record_gives_none() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(read_in(tmp.path(), "nope"), None);
        fs::write(tmp.path().join("bad.json"), "{not json").unwrap();
        assert_eq!(read_in(tmp.path(), "bad"), None);
    }

    #[test]
    fn unsafe_ids_are_refused() {
        let tmp = TempDir::new().unwrap();
        for id in ["", "../x", "a/b", "a b"] {
            assert!(!is_session_id(id), "{id:?}");
            assert_eq!(read_in(tmp.path(), id), None, "{id:?}");
            assert!(lock_in(tmp.path(), id).is_err(), "{id:?}");
            assert!(
                write_in(tmp.path(), &new_record(id), &quiet).is_err(),
                "{id:?}"
            );
        }
        assert!(names(tmp.path()).is_empty());
    }

    #[test]
    fn safe_ids_are_accepted() {
        for id in ["s1", "a-b_C9", "6f1d2c9e-0b7a-4d63-8f0e-1a2b3c4d5e6f"] {
            assert!(is_session_id(id), "{id:?}");
        }
    }

    #[test]
    fn dead_sessions_are_dropped_and_deleted() {
        let tmp = TempDir::new().unwrap();
        write_in(tmp.path(), &live_record("alive"), &quiet).unwrap();
        let dead = Record {
            pid_start: Some(u64::MAX),
            ..live_record("dead")
        };
        write_in(tmp.path(), &dead, &quiet).unwrap();
        let all = load_all_in(tmp.path(), 0.0, &quiet);
        assert_eq!(ids(&all), ["alive"]);
        assert_eq!(read_in(tmp.path(), "dead"), None);
    }

    #[test]
    fn a_dead_session_ends_in_the_journal() {
        let tmp = TempDir::new().unwrap();
        let dead = Record {
            pid_start: Some(u64::MAX),
            ..live_record("dead")
        };
        write_in(tmp.path(), &dead, &quiet).unwrap();
        let events = journaled(|j| {
            load_all_in(tmp.path(), 0.0, j);
        });
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "dead");
        assert_eq!(events[0].status.as_deref(), Some(ENDED));
    }

    #[test]
    fn locks_left_by_ended_sessions_are_swept() {
        let tmp = TempDir::new().unwrap();
        let lock = tmp.path().join("gone.json.lock");
        let file = fs::File::create(&lock).unwrap();
        let mtime = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        file.set_modified(mtime).unwrap();
        load_all_in(tmp.path(), 1_700_000_001.0, &quiet);
        assert!(lock.exists());
        load_all_in(
            tmp.path(),
            1_700_000_000.0 + STALE_LOCK_SECONDS + 1.0,
            &quiet,
        );
        assert!(!lock.exists());
    }

    #[test]
    fn the_lock_of_a_live_record_is_never_swept() {
        let tmp = TempDir::new().unwrap();
        write_in(tmp.path(), &live_record("s1"), &quiet).unwrap();
        drop(lock_in(tmp.path(), "s1").unwrap());
        load_all_in(tmp.path(), 1e12, &quiet);
        assert_eq!(names(tmp.path()), ["s1.json", "s1.json.lock"]);
    }

    #[test]
    fn load_all_skips_the_limits_file() {
        let tmp = TempDir::new().unwrap();
        write_in(tmp.path(), &live_record("s1"), &quiet).unwrap();
        drop(lock_in(tmp.path(), "s1").unwrap());
        fs::write(
            tmp.path().join(LIMITS_FILE),
            r#"{"five_hour": {"pct": 1, "resets_at": 2}}"#,
        )
        .unwrap();
        assert_eq!(ids(&load_all_in(tmp.path(), 0.0, &quiet)), ["s1"]);
        let other = Record {
            pid: Some(-1),
            pid_start: Some(1),
            ..new_record("s2")
        };
        remove_replaced_in(tmp.path(), &other, &quiet);
        assert_eq!(
            names(tmp.path()),
            ["limits.json", "s1.json", "s1.json.lock"]
        );
    }

    #[test]
    fn load_all_ignores_files_that_are_not_named_like_records() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a b.json"), "{}").unwrap();
        fs::write(tmp.path().join("notes.txt"), "x").unwrap();
        assert!(load_all_in(tmp.path(), 0.0, &quiet).is_empty());
        assert_eq!(names(tmp.path()), ["a b.json", "notes.txt"]);
    }

    #[test]
    fn load_all_orders_by_status_then_by_longest_in_it() {
        let tmp = TempDir::new().unwrap();
        for (id, status, since) in [
            ("idle", Status::Idle, 5.0),
            ("late", Status::Permission, 20.0),
            ("early", Status::Permission, 10.0),
            ("done", Status::Done, 1.0),
        ] {
            let r = Record {
                status,
                status_since: since,
                ..live_record(id)
            };
            write_in(tmp.path(), &r, &quiet).unwrap();
        }
        let all = load_all_in(tmp.path(), 0.0, &quiet);
        assert_eq!(ids(&all), ["early", "late", "done", "idle"]);
    }

    #[test]
    fn a_new_record_goes_to_the_journal_at_its_status_time() {
        let tmp = TempDir::new().unwrap();
        let r = Record {
            status: Status::Working,
            status_since: 123.5,
            ..live_record("s1")
        };
        let events = journaled(|j| write_in(tmp.path(), &r, j).unwrap());
        assert_eq!(events.len(), 1);
        assert_eq!((events[0].t, events[0].id.as_str()), (123.5, "s1"));
        assert_eq!(events[0].status.as_deref(), Some("working"));
    }

    #[test]
    fn a_record_without_a_time_is_journaled_at_now() {
        let tmp = TempDir::new().unwrap();
        let events = journaled(|j| write_in(tmp.path(), &live_record("s1"), j).unwrap());
        assert!(events[0].t > 1.7e9);
    }

    #[test]
    fn only_a_status_change_is_journaled() {
        let tmp = TempDir::new().unwrap();
        let first = Record {
            status: Status::Working,
            ..live_record("s1")
        };
        write_in(tmp.path(), &first, &quiet).unwrap();
        let same = Record {
            updated_at: 9.0,
            ..first.clone()
        };
        assert!(journaled(|j| write_in(tmp.path(), &same, j).unwrap()).is_empty());
        let changed = Record {
            status: Status::Done,
            ..first
        };
        let events = journaled(|j| write_in(tmp.path(), &changed, j).unwrap());
        assert_eq!(events[0].status.as_deref(), Some("done"));
    }

    #[test]
    fn removing_a_record_ends_it_in_the_journal_and_drops_its_lock() {
        let tmp = TempDir::new().unwrap();
        write_in(tmp.path(), &live_record("s1"), &quiet).unwrap();
        drop(lock_in(tmp.path(), "s1").unwrap());
        let events = journaled(|j| remove_in(tmp.path(), "s1", false, j));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].status.as_deref(), Some(ENDED));
        assert!(names(tmp.path()).is_empty());
    }

    #[test]
    fn removing_can_keep_the_lock() {
        let tmp = TempDir::new().unwrap();
        write_in(tmp.path(), &live_record("s1"), &quiet).unwrap();
        drop(lock_in(tmp.path(), "s1").unwrap());
        remove_in(tmp.path(), "s1", true, &quiet);
        assert_eq!(names(tmp.path()), ["s1.json.lock"]);
    }

    #[test]
    fn removing_nothing_journals_nothing() {
        let tmp = TempDir::new().unwrap();
        let events = journaled(|j| remove_in(tmp.path(), "s1", false, j));
        assert!(events.is_empty());
    }

    #[test]
    fn removing_an_unsafe_id_does_nothing() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("x.json"), "{}").unwrap();
        let sub = tmp.path().join("sub");
        fs::create_dir(&sub).unwrap();
        remove_in(&sub, "../x", false, &quiet);
        assert!(tmp.path().join("x.json").exists());
    }

    #[test]
    fn a_replaced_session_is_removed_with_its_lock_kept() {
        let tmp = TempDir::new().unwrap();
        let old = live_record("old");
        let new = live_record("new");
        let stranger = Record {
            pid_start: Some(1),
            ..live_record("stranger")
        };
        for r in [&old, &new, &stranger] {
            write_in(tmp.path(), r, &quiet).unwrap();
        }
        let events = journaled(|j| remove_replaced_in(tmp.path(), &new, j));
        // The scan takes the lock of every other session, as the hooks would
        assert_eq!(
            names(tmp.path()),
            [
                "new.json",
                "old.json.lock",
                "stranger.json",
                "stranger.json.lock"
            ]
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "old");
    }

    #[test]
    fn process_start_of_this_process_is_known_and_stable() {
        let pid = std::process::id() as i32;
        assert!(process_start(pid).is_some());
        assert_eq!(process_start(pid), process_start(pid));
    }

    #[test]
    fn the_start_time_is_field_22_after_a_command_name_with_parentheses() {
        let stat = b"42 (we ird) (name) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 777 24 25";
        assert_eq!(parse_start(stat), Some(777));
        assert_eq!(parse_start(b"42 (short) S 1 2"), None);
        assert_eq!(parse_start(b"no parentheses"), None);
    }

    #[test]
    fn process_start_of_a_missing_process_is_none() {
        assert_eq!(process_start(i32::MAX), None);
        assert_eq!(process_start(-1), None);
    }

    #[test]
    fn is_alive_needs_the_same_process() {
        let live = live_record("s1");
        assert!(is_alive(&live));
        let reused = Record {
            pid_start: live.pid_start.map(|s| s + 1),
            ..live.clone()
        };
        assert!(!is_alive(&reused));
        let no_pid = Record { pid: None, ..live };
        assert!(!is_alive(&no_pid));
    }

    #[test]
    fn parallel_writers_lose_no_update() {
        let tmp = TempDir::new().unwrap();
        write_in(tmp.path(), &live_record("s1"), &quiet).unwrap();
        std::thread::scope(|scope| {
            for i in 0..16 {
                let dir = tmp.path();
                scope.spawn(move || {
                    let _lock = lock_in(dir, "s1").unwrap();
                    let mut r = read_in(dir, "s1").unwrap();
                    r.pending.push(Pending {
                        agent: format!("agent{i}"),
                        kind: Waiting::Permission,
                    });
                    write_in(dir, &r, &quiet).unwrap();
                });
            }
        });
        let r = read_in(tmp.path(), "s1").unwrap();
        assert_eq!(r.pending.len(), 16);
    }
}
