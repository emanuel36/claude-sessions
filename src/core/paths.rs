//! Where everything lives on disk, how files are written there, and the error log.
//!
//! Paths are resolved on each call, so the environment variables can change while the process
//! runs. Each public function is a thin wrapper over a private helper that takes an environment
//! lookup, so unit tests never mutate the process environment.

use std::ffi::OsString;
use std::fmt::Display;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use rustix::fs::OFlags;

/// The account's usage limits share the records' directory but are not a session record.
pub const LIMITS_FILE: &str = "limits.json";
/// The error log, in [`state_dir`].
pub const ERROR_LOG: &str = "errors.log";
/// The error log starts over once it grows past this many bytes.
const LOG_BYTES: u64 = 1024 * 1024;
/// Where the fallback runtime directory lives; fixed so every process agrees on it.
const FALLBACK_BASE: &str = "/tmp";

pub(crate) type Lookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

pub(crate) fn process_env(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

/// Returns the variable's value, treating an empty one as unset.
pub(crate) fn var(env: Lookup<'_>, key: &str) -> Option<PathBuf> {
    env(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub(crate) fn home(env: Lookup<'_>) -> PathBuf {
    var(env, "HOME")
        .or_else(std::env::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns the directory that holds the session records and limits, creating it if needed.
///
/// `$CST_RUNTIME_DIR` is taken as it is. Otherwise `$XDG_RUNTIME_DIR/cst` is used, and without
/// it `/tmp/cst-<uid>`, which must be a private directory of this user.
///
/// # Errors
///
/// Fails if the directory cannot be created or, in the `/tmp` fallback, if it is a symlink,
/// belongs to another user, or is writable by group or others: someone else made it first.
pub fn runtime_dir() -> io::Result<PathBuf> {
    let uid = rustix::process::getuid().as_raw();
    runtime_dir_with(&process_env, Path::new(FALLBACK_BASE), uid)
}

/// Returns the directory for what outlives a reboot, such as the saved status line and the
/// error log. It is not created.
pub fn state_dir() -> PathBuf {
    state_dir_with(&process_env)
}

/// Returns the folder where Claude Code keeps its files.
pub fn config_dir() -> PathBuf {
    config_dir_with(&process_env)
}

/// Writes a file atomically, so readers never see a partial one.
///
/// The data goes to a temporary file in the same directory, which then replaces `path`. A failed
/// write leaves no temporary file behind.
///
/// `mode` is the new file's permission bits; with `None` the replaced file's are kept, and a
/// file that did not exist gets `0o600`.
///
/// # Errors
///
/// Fails if the temporary file cannot be written or moved into place.
pub fn write_atomic(path: &Path, data: &[u8], mode: Option<u32>) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    let dir = dir.unwrap_or_else(|| Path::new("."));
    let (mut file, tmp) = create_temp(dir)?;
    let result = (|| {
        file.write_all(data)?;
        let mode = match mode {
            Some(mode) => Some(mode),
            None => fs::metadata(path).ok().map(|m| m.mode()),
        };
        if let Some(mode) = mode {
            file.set_permissions(fs::Permissions::from_mode(mode & 0o7777))?;
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        // Nothing useful to do if the cleanup fails too
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Creates a new private temporary file next to the file it will replace.
fn create_temp(dir: &Path) -> io::Result<(fs::File, PathBuf)> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let mut attempt = 0;
    loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".tmp-{}-{n}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
        {
            Ok(file) => return Ok((file, tmp)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempt < 10 => attempt += 1,
            Err(e) => return Err(e),
        }
    }
}

/// Appends a line to the error log; never fails.
///
/// The log starts over once it passes 1 MiB, so it cannot grow without end. It is opened
/// without following symlinks, so one planted in its place cannot redirect the write.
pub fn log_error(what: &dyn Display) {
    log_error_in(&state_dir(), what);
}

fn runtime_dir_with(env: Lookup<'_>, tmp: &Path, uid: u32) -> io::Result<PathBuf> {
    if let Some(dir) = var(env, "CST_RUNTIME_DIR") {
        make_private(&dir)?;
        return Ok(dir);
    }
    if let Some(runtime) = var(env, "XDG_RUNTIME_DIR") {
        let dir = runtime.join("cst");
        make_private(&dir)?;
        return Ok(dir);
    }
    let dir = tmp.join(format!("cst-{uid}"));
    make_private(&dir)?;
    // lstat: a symlink planted there would lead the records into a folder someone else picked
    let meta = fs::symlink_metadata(&dir)?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o022 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a private folder of this user", dir.display()),
        ));
    }
    Ok(dir)
}

/// Creates `dir` with mode 0700 if it does not exist yet; an existing one is left as it is.
fn make_private(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)?;
    }
    match DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
        result => result,
    }
}

fn state_dir_with(env: Lookup<'_>) -> PathBuf {
    if let Some(dir) = var(env, "CST_STATE_DIR") {
        return dir;
    }
    let base = var(env, "XDG_STATE_HOME").unwrap_or_else(|| home(env).join(".local/state"));
    base.join("cst")
}

fn config_dir_with(env: Lookup<'_>) -> PathBuf {
    var(env, "CLAUDE_CONFIG_DIR").unwrap_or_else(|| home(env).join(".claude"))
}

fn log_error_in(dir: &Path, what: &dyn Display) {
    // Errors while logging an error have nowhere to go
    let _ = try_log_error(dir, what);
}

fn try_log_error(dir: &Path, what: &dyn Display) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(ERROR_LOG);
    let full = fs::metadata(&path).is_ok_and(|m| m.len() > LOG_BYTES);
    let mut options = OpenOptions::new();
    options.write(true).create(true).mode(0o600);
    // A symlink planted in its place must not make the hook write somewhere else
    options.custom_flags(OFlags::NOFOLLOW.bits() as i32);
    if full {
        options.truncate(true);
    } else {
        options.append(true);
    }
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    options
        .open(path)?
        .write_all(format!("{stamp} {what}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};

    use tempfile::TempDir;

    use crate::core::paths::{
        config_dir_with, log_error_in, runtime_dir_with, state_dir_with, write_atomic, ERROR_LOG,
        LIMITS_FILE, LOG_BYTES,
    };

    const MIB_PLUS_ONE: usize = 1024 * 1024 + 1;

    fn env_of(pairs: &[(&str, &std::path::Path)]) -> impl Fn(&str) -> Option<std::ffi::OsString> {
        let map: HashMap<String, std::ffi::OsString> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.as_os_str().to_owned()))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn mode_of(path: &std::path::Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    fn my_uid() -> u32 {
        rustix::process::getuid().as_raw()
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file() {
        let tmp = TempDir::new().unwrap();
        // Moving a file over a directory fails, after the temporary file is written
        let target = tmp.path().join(LIMITS_FILE);
        fs::create_dir(&target).unwrap();
        assert!(write_atomic(&target, b"{}", None).is_err());
        let left: Vec<_> = fs::read_dir(tmp.path()).unwrap().flatten().collect();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].file_name(), LIMITS_FILE);
    }

    #[test]
    fn write_atomic_replaces_the_content() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("a.json");
        write_atomic(&path, b"one", None).unwrap();
        write_atomic(&path, b"two", None).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn write_atomic_makes_a_new_file_private() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("a.json");
        write_atomic(&path, b"x", None).unwrap();
        assert_eq!(mode_of(&path), 0o600);
    }

    #[test]
    fn write_atomic_keeps_mode_of_replaced_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("a.json");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic(&path, b"new", None).unwrap();
        assert_eq!(mode_of(&path), 0o644);
        assert_eq!(fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn write_atomic_uses_the_given_mode_over_the_old_one() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("a.json");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic(&path, b"new", Some(0o755)).unwrap();
        assert_eq!(mode_of(&path), 0o755);
    }

    #[test]
    fn a_runtime_dir_the_user_chose_is_taken_as_it_is() {
        // Made with a umask that lets the group write, as many desktops have
        let tmp = TempDir::new().unwrap();
        let chosen = tmp.path().join("chosen");
        fs::create_dir(&chosen).unwrap();
        fs::set_permissions(&chosen, fs::Permissions::from_mode(0o775)).unwrap();
        let env = env_of(&[("CST_RUNTIME_DIR", &chosen)]);
        assert_eq!(
            runtime_dir_with(&env, tmp.path(), my_uid()).unwrap(),
            chosen
        );
        assert_eq!(mode_of(&chosen), 0o775);
    }

    #[test]
    fn a_chosen_runtime_dir_is_created_private() {
        let tmp = TempDir::new().unwrap();
        let chosen = tmp.path().join("deep/chosen");
        let env = env_of(&[("CST_RUNTIME_DIR", &chosen)]);
        assert_eq!(
            runtime_dir_with(&env, tmp.path(), my_uid()).unwrap(),
            chosen
        );
        assert_eq!(mode_of(&chosen), 0o700);
    }

    #[test]
    fn the_xdg_runtime_dir_gets_a_cst_folder() {
        let tmp = TempDir::new().unwrap();
        let env = env_of(&[("XDG_RUNTIME_DIR", tmp.path())]);
        let dir = runtime_dir_with(&env, tmp.path(), my_uid()).unwrap();
        assert_eq!(dir, tmp.path().join("cst"));
        assert_eq!(mode_of(&dir), 0o700);
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        let tmp = TempDir::new().unwrap();
        let empty = std::path::Path::new("");
        let env = env_of(&[("CST_RUNTIME_DIR", empty), ("XDG_RUNTIME_DIR", tmp.path())]);
        let dir = runtime_dir_with(&env, tmp.path(), my_uid()).unwrap();
        assert_eq!(dir, tmp.path().join("cst"));
    }

    #[test]
    fn without_a_runtime_dir_the_records_go_to_a_folder_of_this_user() {
        let tmp = TempDir::new().unwrap();
        let uid = my_uid();
        let dir = runtime_dir_with(&env_of(&[]), tmp.path(), uid).unwrap();
        assert_eq!(dir, tmp.path().join(format!("cst-{uid}")));
        assert_eq!(mode_of(&dir), 0o700);
    }

    #[test]
    fn fallback_reuses_a_folder_it_made_before() {
        let tmp = TempDir::new().unwrap();
        let first = runtime_dir_with(&env_of(&[]), tmp.path(), my_uid()).unwrap();
        let second = runtime_dir_with(&env_of(&[]), tmp.path(), my_uid()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn fallback_refuses_group_writable() {
        let tmp = TempDir::new().unwrap();
        let uid = my_uid();
        let dir = tmp.path().join(format!("cst-{uid}"));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o770)).unwrap();
        assert!(runtime_dir_with(&env_of(&[]), tmp.path(), uid).is_err());
    }

    #[test]
    fn fallback_refuses_world_writable() {
        let tmp = TempDir::new().unwrap();
        let uid = my_uid();
        let dir = tmp.path().join(format!("cst-{uid}"));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(runtime_dir_with(&env_of(&[]), tmp.path(), uid).is_err());
    }

    #[test]
    fn fallback_refuses_a_folder_of_another_user() {
        // Another user made this user's folder first; here the other user is the one running
        let tmp = TempDir::new().unwrap();
        let victim = my_uid() + 1;
        fs::create_dir(tmp.path().join(format!("cst-{victim}"))).unwrap();
        assert!(runtime_dir_with(&env_of(&[]), tmp.path(), victim).is_err());
    }

    #[test]
    fn fallback_refuses_a_symlink() {
        // Planted by someone else, pointing at a folder that passes the other checks
        let tmp = TempDir::new().unwrap();
        let uid = my_uid();
        let target = tmp.path().join("elsewhere");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&target, tmp.path().join(format!("cst-{uid}"))).unwrap();
        assert!(runtime_dir_with(&env_of(&[]), tmp.path(), uid).is_err());
    }

    #[test]
    fn state_dir_prefers_the_cst_variable() {
        let tmp = TempDir::new().unwrap();
        let env = env_of(&[
            ("CST_STATE_DIR", tmp.path()),
            ("XDG_STATE_HOME", tmp.path()),
        ]);
        assert_eq!(state_dir_with(&env), tmp.path());
    }

    #[test]
    fn state_dir_falls_back_to_xdg_state_home() {
        let tmp = TempDir::new().unwrap();
        let env = env_of(&[("XDG_STATE_HOME", tmp.path()), ("HOME", tmp.path())]);
        assert_eq!(state_dir_with(&env), tmp.path().join("cst"));
    }

    #[test]
    fn state_dir_falls_back_to_the_home_folder() {
        let tmp = TempDir::new().unwrap();
        let env = env_of(&[("HOME", tmp.path())]);
        assert_eq!(state_dir_with(&env), tmp.path().join(".local/state/cst"));
    }

    #[test]
    fn config_dir_prefers_claude_config_dir() {
        let tmp = TempDir::new().unwrap();
        let env = env_of(&[("CLAUDE_CONFIG_DIR", tmp.path()), ("HOME", tmp.path())]);
        assert_eq!(config_dir_with(&env), tmp.path());
    }

    #[test]
    fn config_dir_falls_back_to_dot_claude() {
        let tmp = TempDir::new().unwrap();
        let env = env_of(&[("HOME", tmp.path())]);
        assert_eq!(config_dir_with(&env), tmp.path().join(".claude"));
    }

    #[test]
    fn log_error_writes_a_timestamped_line() {
        let tmp = TempDir::new().unwrap();
        log_error_in(tmp.path(), &"first");
        log_error_in(tmp.path(), &"second");
        let text = fs::read_to_string(tmp.path().join(ERROR_LOG)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        // "YYYY-mm-dd HH:MM:SS " is 20 bytes
        assert!(
            chrono::NaiveDateTime::parse_from_str(&lines[0][..19], "%Y-%m-%d %H:%M:%S").is_ok()
        );
        assert_eq!(&lines[0][19..], " first");
        assert_eq!(&lines[1][19..], " second");
        assert_eq!(mode_of(&tmp.path().join(ERROR_LOG)), 0o600);
    }

    #[test]
    fn log_error_creates_the_state_dir() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("a/b");
        log_error_in(&dir, &"boom");
        assert!(dir.join(ERROR_LOG).is_file());
    }

    #[test]
    fn log_error_truncates_past_one_mib() {
        let tmp = TempDir::new().unwrap();
        let log = tmp.path().join(ERROR_LOG);
        fs::write(&log, "x".repeat(MIB_PLUS_ONE)).unwrap();
        assert!(MIB_PLUS_ONE as u64 > LOG_BYTES);
        log_error_in(tmp.path(), &"boom");
        let text = fs::read_to_string(&log).unwrap();
        assert!(text.ends_with(" boom\n") && !text.starts_with('x'));
    }

    #[test]
    fn log_error_appends_up_to_one_mib() {
        let tmp = TempDir::new().unwrap();
        let log = tmp.path().join(ERROR_LOG);
        fs::write(&log, "x".repeat(LOG_BYTES as usize)).unwrap();
        log_error_in(tmp.path(), &"boom");
        let text = fs::read_to_string(&log).unwrap();
        assert!(text.starts_with('x') && text.ends_with(" boom\n"));
    }

    #[test]
    fn log_error_never_fails() {
        // A file where the state directory should be
        let tmp = TempDir::new().unwrap();
        let blocked = tmp.path().join("blocked");
        fs::write(&blocked, "").unwrap();
        log_error_in(&blocked, &"boom");
    }

    #[test]
    fn log_error_does_not_follow_a_symlink() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("target");
        fs::write(&target, "mine").unwrap();
        symlink(&target, tmp.path().join(ERROR_LOG)).unwrap();
        log_error_in(tmp.path(), &"boom");
        assert_eq!(fs::read_to_string(&target).unwrap(), "mine");
    }
}
