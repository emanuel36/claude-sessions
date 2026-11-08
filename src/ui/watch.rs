//! What a pane watches while it is on screen, and what a change it hears of makes stale.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use crate::claude::history::HISTORY;
use crate::claude::hook::QUIET_SECONDS;
use crate::core::journal::journal_dir;
use crate::core::paths::{self, LIMITS_FILE};
use crate::core::state::{Record, INTERRUPTIBLE};
use crate::ui::pane::LogOnce;
use crate::ui::runtime::Runtime;

/// What a record's file name ends with; its lock file and temporary files do not.
const RECORD_SUFFIX: &str = ".json";

/// What must be read again before the next draw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stale {
    /// The tmux panes and windows.
    pub panes: bool,
    /// The session records.
    pub records: bool,
    /// Claude Code's prompt history.
    pub history: bool,
    /// The journal of the last days.
    pub journal: bool,
    /// The usage limits.
    pub limits: bool,
}

impl Stale {
    /// Nothing.
    pub const NONE: Stale = Stale {
        panes: false,
        records: false,
        history: false,
        journal: false,
        limits: false,
    };
    /// Everything.
    pub const ALL: Stale = Stale {
        panes: true,
        records: true,
        history: true,
        journal: true,
        limits: true,
    };
    /// What the records directory holds.
    const RECORDS_DIR: Stale = Stale {
        records: true,
        limits: true,
        ..Stale::NONE
    };
    /// A record, or a transcript, which tells only whether its session was interrupted: the
    /// records settle that.
    const RECORDS: Stale = Stale {
        records: true,
        ..Stale::NONE
    };
    /// The limits file.
    const LIMITS: Stale = Stale {
        limits: true,
        ..Stale::NONE
    };
    /// The journal's day files.
    const JOURNAL: Stale = Stale {
        journal: true,
        ..Stale::NONE
    };
    /// Claude Code's prompt history.
    const HISTORY: Stale = Stale {
        history: true,
        ..Stale::NONE
    };

    /// Whether anything is stale.
    pub fn any(self) -> bool {
        self != Stale::NONE
    }

    /// Adds what `other` makes stale.
    pub fn add(&mut self, other: Stale) {
        self.panes |= other.panes;
        self.records |= other.records;
        self.history |= other.history;
        self.journal |= other.journal;
        self.limits |= other.limits;
    }
}

/// The directories the panes read from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Dirs {
    /// The records and the limits.
    records: PathBuf,
    /// The journal's day files.
    journal: PathBuf,
    /// Claude Code's folder, which holds `history.jsonl`.
    config: PathBuf,
}

impl Dirs {
    /// Resolves the directories; the records directory is created if it is missing.
    fn resolve() -> io::Result<Self> {
        Ok(Dirs {
            records: paths::runtime_dir()?,
            journal: journal_dir(),
            config: paths::config_dir(),
        })
    }

    /// The directories always watched, with what each holds.
    fn roots(&self) -> [(&Path, Stale); 3] {
        [
            (&self.records, Stale::RECORDS_DIR),
            (&self.journal, Stale::JOURNAL),
            (&self.config, Stale::HISTORY),
        ]
    }

    /// What a change at `path` makes stale, given the records last read.
    ///
    /// A watched directory itself stands for all it holds: its events overflowed, or it was
    /// deleted. A transcript counts only for a session past its grace, which the records then
    /// settle.
    fn touched(&self, path: &Path, records: &[Record], now: f64) -> Stale {
        if let Some((_, holds)) = self.roots().into_iter().find(|(dir, _)| *dir == path) {
            return holds;
        }
        let in_dir = |dir: &Path| path.parent() == Some(dir);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if in_dir(&self.records) {
            if name == LIMITS_FILE {
                return Stale::LIMITS;
            }
            if name.ends_with(RECORD_SUFFIX) && !name.starts_with('.') {
                return Stale::RECORDS;
            }
            return Stale::NONE;
        }
        if in_dir(&self.journal) {
            return Stale::JOURNAL;
        }
        if in_dir(&self.config) {
            return if name == HISTORY {
                Stale::HISTORY
            } else {
                Stale::NONE
            };
        }
        let settles = records.iter().filter(|r| interruptible(r)).any(|r| {
            let transcript = r.transcript.as_deref();
            transcript.and_then(Path::parent) == Some(path)
                || (transcript == Some(path) && now - r.updated_at > QUIET_SECONDS)
        });
        if settles {
            Stale::RECORDS
        } else {
            Stale::NONE
        }
    }
}

/// Whether a session can have been interrupted with Esc unseen: no hook reports that, so its
/// transcript is checked once it has had no hook for [`QUIET_SECONDS`].
pub fn interruptible(r: &Record) -> bool {
    INTERRUPTIBLE.contains(&r.status) && r.background == 0
}

/// The directories a pane watches while it is on screen: the records, the journal and Claude
/// Code's folder, and any other it adds, such as transcripts' folders.
#[derive(Debug, Default)]
pub struct Watches {
    /// Where the pane reads from, once it watched the roots.
    dirs: Option<Dirs>,
    /// The directories watched now.
    watching: HashSet<PathBuf>,
}

impl Watches {
    /// Watches the records, the journal and Claude Code's folder, those not watched yet.
    ///
    /// Returns what the newly watched ones hold, to be read again: their changes were missed
    /// while they were not watched (or were deleted).
    pub fn roots(&mut self, runtime: &mut Runtime, log: &mut LogOnce) -> Stale {
        let dirs = match Dirs::resolve() {
            Ok(dirs) => dirs,
            Err(e) => {
                log.log(&format_args!("cannot use the records folder: {e}"));
                return Stale::NONE;
            }
        };
        let mut stale = Stale::NONE;
        for (dir, holds) in dirs.roots() {
            if self.watch(runtime, dir, log) {
                stale.add(holds);
            }
        }
        self.dirs = Some(dirs);
        stale
    }

    /// Watches a directory unless it is watched; returns whether it was not.
    ///
    /// A directory that does not exist yet is tried again on the next call.
    pub fn watch(&mut self, runtime: &mut Runtime, dir: &Path, log: &mut LogOnce) -> bool {
        if self.watching.contains(dir) {
            return false;
        }
        match runtime.watch(dir) {
            Ok(()) => {
                self.watching.insert(dir.to_path_buf());
                true
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => {
                let shown = dir.display();
                log.log(&format_args!("cannot watch {shown}: {e}"));
                false
            }
        }
    }

    /// What a change at `path` makes stale, given the records last read; a watched directory
    /// that reported itself is watched again on the next [`Watches::roots`] or
    /// [`Watches::watch`].
    pub fn changed(&mut self, path: &Path, records: &[Record], now: f64) -> Stale {
        self.watching.remove(path);
        self.dirs
            .as_ref()
            .map_or(Stale::NONE, |dirs| dirs.touched(path, records, now))
    }

    /// Drops every watch, as a pane does when it leaves the screen.
    pub fn clear(&mut self, runtime: &mut Runtime) {
        runtime.unwatch_all();
        self.watching.clear();
    }

    /// Whether nothing is watched.
    pub fn is_empty(&self) -> bool {
        self.watching.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::core::state::{Record, Status};
    use crate::ui::watch::{Dirs, Stale};

    const NOW: f64 = 1_700_000_000.0;

    fn record(id: &str, status: Status) -> Record {
        Record {
            id: id.to_owned(),
            status,
            cwd: format!("/w/{id}"),
            updated_at: NOW - 1.0,
            transcript: Some(PathBuf::from(format!("/p/{id}/t.jsonl"))),
            ..Record::default()
        }
    }

    fn dirs() -> Dirs {
        Dirs {
            records: PathBuf::from("/run/cst"),
            journal: PathBuf::from("/state/journal"),
            config: PathBuf::from("/home/.claude"),
        }
    }

    fn only(set: impl Fn(&mut Stale)) -> Stale {
        let mut stale = Stale::NONE;
        set(&mut stale);
        stale
    }

    #[test]
    fn a_record_written_makes_the_records_stale() {
        let got = dirs().touched(Path::new("/run/cst/s1.json"), &[], NOW);
        assert_eq!(got, only(|s| s.records = true));
    }

    #[test]
    fn temp_and_lock_files_change_nothing() {
        for name in [".tmp-12-0", "s1.json.lock"] {
            let path = Path::new("/run/cst").join(name);
            assert_eq!(dirs().touched(&path, &[], NOW), Stale::NONE, "{name}");
        }
    }

    #[test]
    fn the_limits_file_makes_the_limits_stale() {
        let got = dirs().touched(Path::new("/run/cst/limits.json"), &[], NOW);
        assert_eq!(got, only(|s| s.limits = true));
    }

    #[test]
    fn a_journal_day_makes_the_journal_stale() {
        let got = dirs().touched(Path::new("/state/journal/2026-11-08.jsonl"), &[], NOW);
        assert_eq!(got, only(|s| s.journal = true));
    }

    #[test]
    fn only_the_history_file_of_the_config_dir_counts() {
        let got = dirs().touched(Path::new("/home/.claude/history.jsonl"), &[], NOW);
        assert_eq!(got, only(|s| s.history = true));
        let other = dirs().touched(Path::new("/home/.claude/settings.json"), &[], NOW);
        assert_eq!(other, Stale::NONE);
    }

    #[test]
    fn a_watched_directory_itself_makes_its_sources_stale() {
        // It overflowed, or was deleted and must be watched again
        let records = dirs().touched(Path::new("/run/cst"), &[], NOW);
        assert_eq!(
            records,
            only(|s| {
                s.records = true;
                s.limits = true;
            })
        );
        let journal = dirs().touched(Path::new("/state/journal"), &[], NOW);
        assert_eq!(journal, only(|s| s.journal = true));
        let config = dirs().touched(Path::new("/home/.claude"), &[], NOW);
        assert_eq!(config, only(|s| s.history = true));
        let transcripts = dirs().touched(Path::new("/p/a"), &[record("a", Status::Working)], NOW);
        assert_eq!(transcripts, only(|s| s.records = true));
    }

    #[test]
    fn a_transcript_counts_only_past_its_grace() {
        let path = Path::new("/p/a/t.jsonl");
        let fresh = record("a", Status::Working);
        assert_eq!(
            dirs().touched(path, std::slice::from_ref(&fresh), NOW),
            Stale::NONE
        );
        let quiet = Record {
            updated_at: NOW - 3.5,
            ..fresh
        };
        assert_eq!(
            dirs().touched(path, std::slice::from_ref(&quiet), NOW),
            only(|s| s.records = true)
        );
        let idle = Record {
            status: Status::Idle,
            ..quiet.clone()
        };
        assert_eq!(dirs().touched(path, &[idle], NOW), Stale::NONE);
        let background = Record {
            background: 1,
            ..quiet
        };
        assert_eq!(dirs().touched(path, &[background], NOW), Stale::NONE);
    }
}
