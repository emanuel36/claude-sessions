//! The history of the day: status changes and costs, one JSON line per event.
//!
//! Events go to `journal/YYYY-MM-DD.jsonl` under the state directory, by the local date of
//! their time, so the sidebar's summary survives sessions closing and the sidebar restarting.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::core::paths;

/// Day files older than this many days are deleted.
pub const KEEP_DAYS: u64 = 7;
/// The status a session leaves the journal with when its record goes away; not a live status.
pub const ENDED: &str = "ended";
/// The model of cost events written before the journal recorded one.
pub const UNKNOWN_MODEL: &str = "unknown";
const SUFFIX: &str = ".jsonl";

/// One line of the journal: a status change or a cost reading of one session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JournalEvent {
    /// Unix timestamp, in seconds.
    pub t: f64,
    /// The session the event belongs to.
    #[serde(default)]
    pub id: String,
    /// A [`crate::core::state::Status`] name, or [`ENDED`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The session's running cost in dollars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// The session's running count of lines added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<u64>,
    /// The session's running count of lines removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<u64>,
    /// The model that produced the cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// The directory of the day files.
pub fn journal_dir() -> PathBuf {
    paths::state_dir().join("journal")
}

fn day_path(dir: &Path, day: NaiveDate) -> PathBuf {
    dir.join(format!("{day}{SUFFIX}"))
}

/// The local date of a Unix timestamp, or `None` for one that is not a date.
fn day_of(t: f64) -> Option<NaiveDate> {
    if !t.is_finite() {
        return None;
    }
    let secs = t.floor();
    let nanos = ((t - secs) * 1e9) as u32;
    let utc = DateTime::from_timestamp(secs as i64, nanos.min(999_999_999))?;
    Some(utc.with_timezone(&Local).date_naive())
}

/// Adds an event to the file of its day, and deletes the files of past weeks.
///
/// Never fails: the hook and the status line call it, and losing an event is better than
/// breaking them. A failure goes to the error log.
pub fn append(e: &JournalEvent) {
    let Some(day) = day_of(e.t) else {
        paths::log_error(&format_args!("journal: event time {} is not a date", e.t));
        return;
    };
    let today = Local::now().date_naive();
    if let Err(err) = append_in(&journal_dir(), e, day, today) {
        paths::log_error(&format_args!("journal: cannot append: {err}"));
    }
}

fn append_in(dir: &Path, e: &JournalEvent, day: NaiveDate, today: NaiveDate) -> io::Result<()> {
    let mut line = serde_json::to_vec(e).map_err(io::Error::other)?;
    line.push(b'\n');
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let path = day_path(dir, day);
    // Old files only need a look when a day starts a file, not on every event
    let started = !path.exists();
    // One write on an O_APPEND fd: lines from concurrent hooks never interleave
    OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&path)?
        .write_all(&line)?;
    if started {
        prune(dir, today - chrono::Days::new(KEEP_DAYS))?;
    }
    Ok(())
}

/// Deletes the day files from before a date.
fn prune(dir: &Path, oldest: NaiveDate) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let name = entry?.file_name();
        let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(SUFFIX)) else {
            continue;
        };
        let Ok(day) = stem.parse::<NaiveDate>() else {
            continue;
        };
        if day < oldest {
            match fs::remove_file(dir.join(&name)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Parses complete lines, skipping the ones that are not an event.
fn parse(raw: &[u8], events: &mut Vec<JournalEvent>) -> usize {
    let before = events.len();
    for line in raw.split(|&b| b == b'\n') {
        // A JSON array would otherwise deserialize as a struct's fields in order
        if line.trim_ascii_start().first() != Some(&b'{') {
            continue;
        }
        if let Ok(event) = serde_json::from_slice(line) {
            events.push(event);
        }
    }
    events.len() - before
}

/// Reads a day's events again and again, parsing only the lines added since the last read.
///
/// The sidebar reads the day every second; a busy day's file takes long to parse whole.
#[derive(Debug, Default)]
pub struct DayReader {
    /// The day, device and inode of the file the offset belongs to.
    file: Option<(NaiveDate, u64, u64)>,
    offset: u64,
    events: Vec<JournalEvent>,
    generation: u64,
}

impl DayReader {
    /// Returns a reader that has read nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// A number that changes only when a read parsed new events or dropped the old ones, so
    /// callers can tell nothing changed without comparing the events.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The events of the last read, without reading the file again.
    pub fn events(&self) -> &[JournalEvent] {
        &self.events
    }

    /// Returns the events of a day in `dir`, in the order they were written.
    ///
    /// Lines that do not parse (one torn by a crash mid-write) are skipped. A line still being
    /// written (no newline yet) waits for the next read. The reading starts over for another
    /// day, another file, or a file shorter than what was read.
    pub(crate) fn read_in(&mut self, dir: &Path, day: NaiveDate) -> &[JournalEvent] {
        match File::open(day_path(dir, day)) {
            // A file that cannot be read keeps what was read; the sidebar asks again each second,
            // so the error is not logged
            Ok(file) => drop(self.catch_up(file, day)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => self.reset(None),
            Err(_) => {}
        }
        &self.events
    }

    fn reset(&mut self, file: Option<(NaiveDate, u64, u64)>) {
        self.file = file;
        self.offset = 0;
        if !self.events.is_empty() {
            self.events.clear();
            self.generation += 1;
        }
    }

    fn catch_up(&mut self, mut file: File, day: NaiveDate) -> io::Result<()> {
        let meta = file.metadata()?;
        let id = Some((day, meta.dev(), meta.ino()));
        if id != self.file || meta.len() < self.offset {
            self.reset(id);
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut raw = Vec::new();
        file.read_to_end(&mut raw)?;
        // The last line may still be being written: it waits for its newline
        let Some(end) = raw.iter().rposition(|&b| b == b'\n') else {
            return Ok(());
        };
        self.offset += end as u64 + 1;
        if parse(&raw[..=end], &mut self.events) > 0 {
            self.generation += 1;
        }
        Ok(())
    }
}

/// The events of each day, in local time.
pub type Days<'a> = BTreeMap<NaiveDate, &'a [JournalEvent]>;

/// What a session added over a window: cost in dollars and lines of code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SessionTotals {
    /// Dollars.
    pub cost: f64,
    /// Lines added.
    pub added: u64,
    /// Lines removed.
    pub removed: u64,
}

impl SessionTotals {
    fn of(e: &JournalEvent) -> Self {
        Self {
            cost: e.cost.unwrap_or(0.0),
            added: e.added.unwrap_or(0),
            removed: e.removed.unwrap_or(0),
        }
    }
}

/// The cost and lines each session added from `first` on.
///
/// The values are running totals, so a session that began earlier counts only what it grew by:
/// its highest value in the window minus its last value before it (a count that restarted lower
/// than that gives 0 lines, not a negative number). Only sessions with a cost in the window
/// appear; an event without `added` or `removed` counts as 0.
pub fn window_totals(days: &Days, first: NaiveDate) -> HashMap<String, SessionTotals> {
    let mut before: HashMap<&str, SessionTotals> = HashMap::new();
    let mut peak: HashMap<&str, SessionTotals> = HashMap::new();
    for (&day, &events) in days {
        for e in events.iter().filter(|e| e.cost.is_some()) {
            let now = SessionTotals::of(e);
            if day < first {
                before.insert(&e.id, now);
            } else {
                let top = peak.entry(&e.id).or_default();
                top.cost = top.cost.max(now.cost);
                top.added = top.added.max(now.added);
                top.removed = top.removed.max(now.removed);
            }
        }
    }
    peak.into_iter()
        .map(|(id, top)| {
            let base = before.get(id).copied().unwrap_or_default();
            let grown = SessionTotals {
                cost: top.cost - base.cost,
                added: top.added.saturating_sub(base.added),
                removed: top.removed.saturating_sub(base.removed),
            };
            (id.to_owned(), grown)
        })
        .collect()
}

/// The cost all sessions added from `first` on, by the model each event names.
///
/// Costs are running totals, counted as [`window_totals`] counts them: from a session's last
/// cost before the window, each new high goes to the model of the event that reaches it. Events
/// without a model count under [`UNKNOWN_MODEL`].
pub fn model_costs(days: &Days, first: NaiveDate) -> HashMap<String, f64> {
    let mut before: HashMap<&str, f64> = HashMap::new();
    let mut high: HashMap<&str, f64> = HashMap::new();
    let mut totals: HashMap<String, f64> = HashMap::new();
    for (&day, &events) in days {
        for e in events {
            let Some(cost) = e.cost else { continue };
            if day < first {
                before.insert(&e.id, cost);
                continue;
            }
            let top = high
                .get(e.id.as_str())
                .or(before.get(e.id.as_str()))
                .copied();
            let top = top.unwrap_or(0.0);
            if cost > top {
                let model = e.model.as_deref().filter(|m| !m.is_empty());
                *totals
                    .entry(model.unwrap_or(UNKNOWN_MODEL).to_owned())
                    .or_default() += cost - top;
                high.insert(&e.id, cost);
            }
        }
    }
    totals
}

/// The cost all sessions added on each day.
///
/// Costs are running totals: a session's share of a day is its highest cost that day minus its
/// last cost before it. Every day given has an entry.
pub fn day_costs(days: &Days) -> BTreeMap<NaiveDate, f64> {
    let mut last: HashMap<&str, f64> = HashMap::new();
    let mut totals = BTreeMap::new();
    for (&day, &events) in days {
        // Sessions in the order they first appear, so the sum does not depend on hashing
        let mut slot: HashMap<&str, usize> = HashMap::new();
        let mut peak: Vec<(&str, f64)> = Vec::new();
        for e in events {
            let Some(cost) = e.cost else { continue };
            let i = *slot.entry(&e.id).or_insert_with(|| {
                peak.push((&e.id, 0.0));
                peak.len() - 1
            });
            peak[i].1 = peak[i].1.max(cost);
        }
        // Folded from +0.0: an empty f64 sum is -0.0, which formats as $-0.00
        let total = peak
            .iter()
            .map(|&(id, top)| (top - last.get(id).copied().unwrap_or(0.0)).max(0.0))
            .fold(0.0, |sum, v| sum + v);
        totals.insert(day, total);
        last.extend(peak);
    }
    totals
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Arc;
    use std::thread;

    use chrono::{Days as ChronoDays, Local, NaiveDate, TimeZone};
    use tempfile::TempDir;

    use crate::core::journal::{
        append_in, day_costs, day_of, day_path, model_costs, window_totals, DayReader, Days,
        JournalEvent, SessionTotals, ENDED,
    };

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Noon of a local day: events a minute apart stay in that day's file.
    fn noon(day: NaiveDate) -> f64 {
        let t = day.and_hms_opt(12, 0, 0).unwrap();
        Local.from_local_datetime(&t).unwrap().timestamp() as f64
    }

    fn status(t: f64, id: &str, s: &str) -> JournalEvent {
        JournalEvent {
            t,
            id: id.into(),
            status: Some(s.into()),
            ..Default::default()
        }
    }

    fn cost(t: f64, id: &str, c: f64) -> JournalEvent {
        JournalEvent {
            t,
            id: id.into(),
            cost: Some(c),
            ..Default::default()
        }
    }

    fn put(dir: &Path, e: &JournalEvent) {
        let day = day_of(e.t).unwrap();
        append_in(dir, e, day, day).unwrap();
    }

    fn read_day(dir: &Path, day: NaiveDate) -> Vec<JournalEvent> {
        DayReader::new().read_in(dir, day).to_vec()
    }

    fn days(pairs: &[(NaiveDate, Vec<JournalEvent>)]) -> Days<'_> {
        pairs.iter().map(|(d, e)| (*d, e.as_slice())).collect()
    }

    fn open_append(dir: &Path, day: NaiveDate) -> fs::File {
        OpenOptions::new()
            .append(true)
            .open(day_path(dir, day))
            .unwrap()
    }

    fn costs(reader: &mut DayReader, dir: &Path, day: NaiveDate) -> Vec<Option<f64>> {
        reader.read_in(dir, day).iter().map(|e| e.cost).collect()
    }

    #[test]
    fn day_of_is_the_local_date() {
        let day = date(2026, 11, 2);
        assert_eq!(day_of(noon(day)), Some(day));
        assert_eq!(day_of(f64::INFINITY), None);
        assert_eq!(day_of(f64::NAN), None);
        assert_eq!(day_of(1e300), None);
    }

    #[test]
    fn event_serializes_without_its_empty_fields() {
        let line = serde_json::to_string(&status(1.5, "s1", "working")).unwrap();
        assert_eq!(line, r#"{"t":1.5,"id":"s1","status":"working"}"#);
        let line = serde_json::to_string(&cost(2.0, "s1", 1.5)).unwrap();
        assert_eq!(line, r#"{"t":2.0,"id":"s1","cost":1.5}"#);
    }

    #[test]
    fn a_session_that_ends_logs_its_end() {
        let tmp = TempDir::new().unwrap();
        let day = date(2026, 11, 2);
        let t = noon(day);
        put(tmp.path(), &status(t, "s1", "permission"));
        put(tmp.path(), &status(t + 60.0, "s1", ENDED));
        assert_eq!(
            read_day(tmp.path(), day),
            vec![
                status(t, "s1", "permission"),
                status(t + 60.0, "s1", "ended")
            ]
        );
    }

    #[test]
    fn read_day_skips_torn_line() {
        let tmp = TempDir::new().unwrap();
        let day = date(2026, 11, 2);
        let t = noon(day);
        put(tmp.path(), &status(t, "a", "working"));
        open_append(tmp.path(), day)
            .write_all(b"{\"t\": 1.0, \"id\": \"a\", \"sta\n[1, 2]\n[1.0, \"a\"]\n\n")
            .unwrap();
        put(tmp.path(), &cost(t + 1.0, "a", 1.5));
        assert_eq!(
            read_day(tmp.path(), day),
            vec![status(t, "a", "working"), cost(t + 1.0, "a", 1.5)]
        );
    }

    #[test]
    fn read_day_missing_dir_is_empty() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("nope");
        assert!(read_day(&missing, date(2026, 11, 2)).is_empty());
    }

    #[test]
    fn old_files_are_pruned_when_a_day_starts_a_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let today = date(2026, 11, 8);
        let old = today - ChronoDays::new(8);
        let kept = today - ChronoDays::new(7);
        for day in [old, kept, today] {
            fs::write(
                day_path(dir, day),
                "{\"t\":1.0,\"id\":\"a\",\"cost\":1.0}\n",
            )
            .unwrap();
        }
        // Only the append that starts a file prunes
        append_in(dir, &cost(noon(today), "a", 2.0), today, today).unwrap();
        assert!(day_path(dir, old).exists());
        // A late event of two days ago starts a file, and the age counts from today
        let late = today - ChronoDays::new(2);
        append_in(dir, &cost(noon(late), "a", 2.0), late, today).unwrap();
        assert!(!day_path(dir, old).exists());
        assert!(day_path(dir, kept).exists() && day_path(dir, today).exists());
    }

    #[test]
    fn pruning_leaves_files_that_are_not_day_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let today = date(2026, 11, 8);
        fs::write(dir.join("notes.txt"), "x").unwrap();
        fs::write(dir.join("old.jsonl"), "x").unwrap();
        append_in(dir, &cost(noon(today), "a", 1.0), today, today).unwrap();
        assert!(dir.join("notes.txt").exists() && dir.join("old.jsonl").exists());
    }

    #[test]
    fn append_creates_a_private_directory_and_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("journal");
        let day = date(2026, 11, 2);
        append_in(&dir, &cost(noon(day), "a", 1.0), day, day).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&day_path(&dir, day)), 0o600);
    }

    #[test]
    fn append_reports_an_unwritable_directory_instead_of_panicking() {
        let tmp = TempDir::new().unwrap();
        let blocker = tmp.path().join("file");
        fs::write(&blocker, "").unwrap();
        let day = date(2026, 11, 2);
        assert!(append_in(&blocker.join("journal"), &cost(1.0, "a", 1.0), day, day).is_err());
    }

    #[test]
    fn reader_parses_only_what_was_appended() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let day = date(2026, 11, 2);
        let start = noon(day);
        let mut reader = DayReader::new();
        put(dir, &cost(start, "a", 1.0));
        assert_eq!(reader.read_in(dir, day), [cost(start, "a", 1.0)]);
        // Rewritten in place at the same size: a reader that started over would see "b"
        let path = day_path(dir, day);
        fs::write(
            &path,
            fs::read_to_string(&path).unwrap().replace("\"a\"", "\"b\""),
        )
        .unwrap();
        put(dir, &cost(start + 1.0, "a", 2.0));
        let ids: Vec<&str> = reader
            .read_in(dir, day)
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(ids, ["a", "a"]);
    }

    #[test]
    fn reader_waits_for_a_line_without_newline() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let day = date(2026, 11, 2);
        let start = noon(day);
        let mut reader = DayReader::new();
        put(dir, &cost(start, "a", 1.0));
        let line = serde_json::to_string(&cost(start + 1.0, "a", 2.0)).unwrap();
        let mut f = open_append(dir, day);
        f.write_all(&line.as_bytes()[..10]).unwrap();
        assert_eq!(reader.read_in(dir, day).len(), 1);
        let before = reader.generation();
        // Even a complete JSON value waits for its newline
        f.write_all(&line.as_bytes()[10..]).unwrap();
        assert_eq!(reader.read_in(dir, day).len(), 1);
        assert_eq!(reader.generation(), before);
        f.write_all(b"\n").unwrap();
        assert_eq!(costs(&mut reader, dir, day), [Some(1.0), Some(2.0)]);
        assert_ne!(reader.generation(), before);
    }

    #[test]
    fn reader_skips_a_torn_line() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let day = date(2026, 11, 2);
        let start = noon(day);
        let mut reader = DayReader::new();
        put(dir, &cost(start, "a", 1.0));
        assert_eq!(reader.read_in(dir, day).len(), 1);
        open_append(dir, day)
            .write_all(b"{\"t\": 5.0, \"id\"\n")
            .unwrap();
        put(dir, &cost(start + 2.0, "a", 3.0));
        assert_eq!(costs(&mut reader, dir, day), [Some(1.0), Some(3.0)]);
    }

    #[test]
    fn reader_restarts_when_the_file_shrinks_or_is_replaced() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let day = date(2026, 11, 2);
        let start = noon(day);
        let mut reader = DayReader::new();
        put(dir, &cost(start, "a", 1.0));
        put(dir, &cost(start + 1.0, "a", 2.0));
        assert_eq!(reader.read_in(dir, day).len(), 2);
        // Shrunk: same file, fewer bytes than were read
        let line = |id: &str| serde_json::to_string(&cost(start, id, 3.0)).unwrap() + "\n";
        fs::write(day_path(dir, day), line("b")).unwrap();
        assert_eq!(reader.read_in(dir, day), [cost(start, "b", 3.0)]);
        // Replaced by a longer file: only a new inode tells
        let replacement = dir.join("replacement");
        let two = line("b") + &line("c");
        fs::write(&replacement, two.repeat(2)).unwrap();
        fs::rename(&replacement, day_path(dir, day)).unwrap();
        let ids: Vec<&str> = reader
            .read_in(dir, day)
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(ids, ["b", "c", "b", "c"]);
        // Another day, then back
        assert!(reader.read_in(dir, day + ChronoDays::new(1)).is_empty());
        assert_eq!(reader.read_in(dir, day).len(), 4);
    }

    #[test]
    fn generation_bumps_only_when_the_events_change() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let day = date(2026, 11, 2);
        let mut reader = DayReader::new();
        assert!(reader.read_in(dir, day).is_empty());
        let empty = reader.generation();
        assert!(reader.read_in(dir, day).is_empty());
        assert_eq!(reader.generation(), empty);
        put(dir, &cost(noon(day), "a", 1.0));
        reader.read_in(dir, day);
        let one = reader.generation();
        assert_ne!(one, empty);
        reader.read_in(dir, day);
        assert_eq!(reader.generation(), one);
        // The file going away empties the events
        fs::remove_file(day_path(dir, day)).unwrap();
        assert!(reader.read_in(dir, day).is_empty());
        let gone = reader.generation();
        assert_ne!(gone, one);
        assert!(reader.read_in(dir, day).is_empty());
        assert_eq!(reader.generation(), gone);
    }

    #[test]
    fn concurrent_appends_never_interleave() {
        let tmp = TempDir::new().unwrap();
        let dir = Arc::new(tmp.path().to_path_buf());
        let day = date(2026, 11, 2);
        let start = noon(day);
        let handles: Vec<_> = (0..8)
            .map(|n| {
                let dir = Arc::clone(&dir);
                thread::spawn(move || {
                    for i in 0..500 {
                        let mut e =
                            cost(start + f64::from(i), &format!("thread-{n}"), f64::from(i));
                        e.model = Some("a model name long enough to make the line sizeable".into());
                        append_in(&dir, &e, day, day).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let raw = fs::read_to_string(day_path(&dir, day)).unwrap();
        assert_eq!(raw.lines().count(), 4000);
        for line in raw.lines() {
            serde_json::from_str::<JournalEvent>(line).unwrap();
        }
        assert_eq!(read_day(&dir, day).len(), 4000);
    }

    #[test]
    fn spend_counts_only_cost_added_in_the_window() {
        let (day1, day2) = (date(2026, 1, 1), date(2026, 1, 2));
        let data = [
            (day1, vec![cost(1.0, "s", 3.0)]),
            (day2, vec![cost(2.0, "s", 4.0), cost(3.0, "s", 5.0)]),
        ];
        assert_eq!(window_totals(&days(&data), day2)["s"].cost, 2.0);
    }

    #[test]
    fn spend_of_a_session_born_in_the_window_is_its_highest_cost() {
        let day = date(2026, 1, 2);
        let data = [(day, vec![cost(1.0, "s", 1.5), status(2.0, "s", "idle")])];
        assert_eq!(window_totals(&days(&data), day)["s"].cost, 1.5);
    }

    #[test]
    fn window_totals_grow_from_before_the_window() {
        let (day1, day2) = (date(2026, 1, 1), date(2026, 1, 2));
        let with_lines = |t, id: &str, c, added, removed| JournalEvent {
            added: Some(added),
            removed: Some(removed),
            ..cost(t, id, c)
        };
        let data = [
            (day1, vec![with_lines(1.0, "a", 1.0, 5, 1)]),
            (
                day2,
                vec![with_lines(2.0, "a", 3.0, 25, 4), cost(3.0, "b", 0.5)],
            ),
        ];
        let totals = window_totals(&days(&data), day2);
        assert_eq!(totals.len(), 2);
        assert_eq!(
            totals["a"],
            SessionTotals {
                cost: 2.0,
                added: 20,
                removed: 3
            }
        );
        assert_eq!(
            totals["b"],
            SessionTotals {
                cost: 0.5,
                added: 0,
                removed: 0
            }
        );
    }

    #[test]
    fn window_totals_never_go_below_zero_lines() {
        let (day1, day2) = (date(2026, 1, 1), date(2026, 1, 2));
        let with_lines = |t, c, added| JournalEvent {
            added: Some(added),
            ..cost(t, "a", c)
        };
        // The session restarted its running totals the day before the window
        let data = [
            (
                day1,
                vec![with_lines(1.0, 9.0, 50), with_lines(2.0, 1.0, 10)],
            ),
            (day2, vec![with_lines(3.0, 2.0, 4)]),
        ];
        let a = window_totals(&days(&data), day2)["a"];
        assert_eq!((a.cost, a.added), (1.0, 0));
    }

    #[test]
    fn day_costs_counts_each_days_growth() {
        let (d0, d1, d2) = (date(2026, 11, 1), date(2026, 11, 2), date(2026, 11, 3));
        let data = [
            (d0, vec![cost(0.0, "a", 1.0)]),
            (d1, vec![cost(0.0, "a", 2.5), cost(0.0, "b", 0.5)]),
            (d2, vec![cost(0.0, "a", 4.0), cost(0.0, "b", 0.5)]),
        ];
        let expected = BTreeMap::from([(d0, 1.0), (d1, 2.0), (d2, 1.5)]);
        assert_eq!(day_costs(&days(&data)), expected);
    }

    #[test]
    fn day_costs_gives_a_day_without_cost_a_zero() {
        let day = date(2026, 11, 1);
        let data = [(day, vec![status(0.0, "a", "idle")])];
        assert_eq!(day_costs(&days(&data)), BTreeMap::from([(day, 0.0)]));
    }

    #[test]
    fn day_costs_gives_an_empty_day_a_positive_zero() {
        let day = date(2026, 11, 1);
        let data = [(day, vec![])];
        let total = day_costs(&days(&data))[&day];
        assert!(total.is_sign_positive(), "{total} formats as {total:.2}");
    }

    #[test]
    fn model_costs_give_each_rise_to_its_event_model() {
        let day = date(2026, 11, 3);
        let before = day - ChronoDays::new(1);
        let modeled = |t, id: &str, c, m: &str| JournalEvent {
            model: Some(m.into()),
            ..cost(t, id, c)
        };
        let data = [
            (before, vec![modeled(1.0, "a", 2.0, "Opus")]),
            (
                day,
                vec![
                    modeled(2.0, "a", 5.0, "Opus"),
                    modeled(3.0, "a", 6.0, "Sonnet"),
                    cost(4.0, "b", 1.5),
                    status(5.0, "a", "done"),
                ],
            ),
        ];
        // Only what rose inside the window, by the model of the event that shows it
        let expected = HashMap::from([
            ("Opus".to_string(), 3.0),
            ("Sonnet".to_string(), 1.0),
            ("unknown".to_string(), 1.5),
        ]);
        assert_eq!(model_costs(&days(&data), day), expected);
    }

    #[test]
    fn model_costs_count_from_the_last_cost_before_the_window() {
        let day = date(2026, 11, 3);
        let before = day - ChronoDays::new(1);
        // The session restarted its running total the day before
        let data = [
            (before, vec![cost(1.0, "a", 10.0), cost(2.0, "a", 1.0)]),
            (
                day,
                vec![JournalEvent {
                    model: Some("Opus".into()),
                    ..cost(3.0, "a", 3.0)
                }],
            ),
        ];
        let d = days(&data);
        assert_eq!(
            model_costs(&d, day),
            HashMap::from([("Opus".to_string(), 2.0)])
        );
        assert_eq!(window_totals(&d, day)["a"].cost, 2.0);
    }
}
