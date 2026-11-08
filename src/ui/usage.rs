//! Spend, lines and activity totals for the sidebar and the landing page.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Days as ChronoDays, Local, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use serde::Deserialize;

use crate::claude::history::HistorySession;
use crate::core::journal::{
    day_costs, journal_dir, model_costs, window_totals, DayReader, Days, SessionTotals, ENDED,
};
use crate::core::state::{Status, NEEDS_YOU};

/// Days of spend the sidebar shows, today included.
pub const WEEK_DAYS: u64 = 7;
/// The longest a span still open at `now` counts, in seconds: a session killed mid-turn writes
/// no event to end it.
pub const MAX_OPEN: f64 = 7200.0;

/// What today's journal adds up to.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Day {
    /// Lines added today.
    pub added: u64,
    /// Lines removed today.
    pub removed: u64,
    /// Sessions that had an event today other than ending.
    pub sessions: u32,
    /// Seconds sessions spent blocked on the user, an approval or an answer.
    pub waited: f64,
}

/// What the journal adds up to over the last week.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Totals {
    /// Dollars spent today.
    pub today: f64,
    /// Dollars by folder name, most expensive first.
    pub by_project: Vec<(String, f64)>,
    /// Dollars by model, most expensive first.
    pub by_model: Vec<(String, f64)>,
    /// Lines added over the week.
    pub added: u64,
    /// Lines removed over the week.
    pub removed: u64,
    /// Sessions that added anything over the week.
    pub sessions: u32,
    /// Dollars of each session over the week.
    pub per_session: HashMap<String, f64>,
    /// What each of the last 7 days cost, oldest first, today last.
    pub days: Vec<(NaiveDate, f64)>,
    /// What each session added from today on, for the lines of the day.
    pub today_by_session: HashMap<String, SessionTotals>,
}

/// What a [`Totals`] was computed from.
#[derive(Debug, PartialEq)]
struct Key {
    days: Vec<NaiveDate>,
    generations: Vec<u64>,
    history_gen: u64,
}

/// Reads the journal's last days, keeping the days' readers between calls.
#[derive(Debug, Default)]
pub struct Reader {
    dir: Option<PathBuf>,
    journals: BTreeMap<NaiveDate, DayReader>,
    key: Option<Key>,
    totals: Totals,
}

impl Reader {
    /// Returns a reader of the journal in the state directory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a reader of the day files in `dir`.
    pub fn in_dir(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            ..Self::default()
        }
    }

    /// Returns the cost, lines and sessions of today and of the last 7 days.
    ///
    /// The result is computed again only when a day's events changed or `history_gen` did.
    ///
    /// # Arguments
    /// * `history`: the sessions of the prompt history, to name each one's folder.
    /// * `history_gen`: a number that changes when `history` does.
    /// * `now`: current time as a Unix timestamp.
    pub fn totals(&mut self, history: &[HistorySession], history_gen: u64, now: f64) -> &Totals {
        self.totals_in(history, history_gen, now, &Local)
    }

    fn totals_in<Tz: TimeZone>(
        &mut self,
        history: &[HistorySession],
        history_gen: u64,
        now: f64,
        tz: &Tz,
    ) -> &Totals {
        let Some(today) = local_date(now, tz) else {
            return &self.totals;
        };
        let days: Vec<NaiveDate> = (0..=WEEK_DAYS)
            .rev()
            .filter_map(|n| today.checked_sub_days(ChronoDays::new(n)))
            .collect();
        let Some(&first) = days.get(1) else {
            return &self.totals;
        };
        self.journals.retain(|d, _| days.contains(d));
        let dir = self.dir.clone().unwrap_or_else(journal_dir);
        for &d in &days {
            self.journals.entry(d).or_default().read_in(&dir, d);
        }
        // A reader's generation moves only when its file grew: nothing new, nothing to redo
        let key = Key {
            days,
            generations: self.journals.values().map(DayReader::generation).collect(),
            history_gen,
        };
        if self.key.as_ref() != Some(&key) {
            self.totals = sum_up(&events_of(&self.journals), history, first, today);
            self.key = Some(key);
        }
        &self.totals
    }

    /// Returns the seconds sessions worked in each hour of today, midnight first.
    ///
    /// Reads what [`Reader::totals`] read last, so call it after `totals` in the same tick.
    pub fn hours(&self, now: f64) -> [f64; 24] {
        self.hours_in(now, &Local)
    }

    fn hours_in<Tz: TimeZone>(&self, now: f64, tz: &Tz) -> [f64; 24] {
        let Some(today) = local_date(now, tz) else {
            return [0.0; 24];
        };
        working_hours_in(&events_of(&self.journals), today, now, tz)
    }

    /// Returns today's lines, sessions and waiting.
    ///
    /// Reads what [`Reader::totals`] read last, so call it after `totals` in the same tick.
    pub fn day(&self, now: f64) -> Day {
        self.day_in(now, &Local)
    }

    fn day_in<Tz: TimeZone>(&self, now: f64, tz: &Tz) -> Day {
        let Some(today) = local_date(now, tz) else {
            return Day::default();
        };
        if !self.journals.contains_key(&today) {
            return Day::default();
        }
        // The lines need the whole week's costs, which `totals` added up once
        let known = &self.totals.today_by_session;
        day_summary(&events_of(&self.journals), today, now, Some(known))
    }
}

fn events_of(journals: &BTreeMap<NaiveDate, DayReader>) -> Days<'_> {
    journals.iter().map(|(&d, r)| (d, r.events())).collect()
}

fn local_date<Tz: TimeZone>(t: f64, tz: &Tz) -> Option<NaiveDate> {
    if !t.is_finite() {
        return None;
    }
    let utc = DateTime::<Utc>::from_timestamp(t.floor() as i64, 0)?;
    Some(utc.with_timezone(tz).date_naive())
}

/// Adds up the window from `first` on: the week's costs, lines and sessions.
fn sum_up(events: &Days, history: &[HistorySession], first: NaiveDate, today: NaiveDate) -> Totals {
    let week = window_totals(events, first);
    let cwds: HashMap<&str, &str> = history
        .iter()
        .map(|s| (s.id.as_str(), s.cwd.as_str()))
        .collect();
    // Sessions in the order they first show a cost, so ties rank the same on every run
    let mut seen = HashSet::new();
    let mut order = Vec::new();
    for e in events.range(first..).flat_map(|(_, evs)| evs.iter()) {
        if e.cost.is_some() && seen.insert(e.id.as_str()) {
            order.push(e.id.as_str());
        }
    }
    let mut projects: Vec<(String, f64)> = Vec::new();
    for sid in order {
        let (Some(t), Some(cwd)) = (week.get(sid), cwds.get(sid).filter(|c| !c.is_empty())) else {
            continue;
        };
        if t.cost <= 0.0 {
            continue;
        }
        let name = cwd.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        match projects.iter_mut().find(|p| p.0 == name) {
            Some(p) => p.1 += t.cost,
            None => projects.push((name.to_owned(), t.cost)),
        }
    }
    let mut models: Vec<(String, f64)> = model_costs(events, first).into_iter().collect();
    // The map has no order of its own: equal costs fall back to the names
    models.sort_by(|a, b| a.0.cmp(&b.0));
    let days: Vec<(NaiveDate, f64)> = day_costs(events)
        .into_iter()
        .filter(|(d, _)| *d >= first)
        .collect();
    Totals {
        // From the same per-day sums as the chart, so the today line and today's column agree
        today: days.last().map_or(0.0, |d| d.1),
        by_project: ranked(projects),
        by_model: ranked(models),
        added: week.values().map(|t| t.added).sum(),
        removed: week.values().map(|t| t.removed).sum(),
        sessions: week
            .values()
            .filter(|t| t.cost != 0.0 || t.added != 0 || t.removed != 0)
            .count() as u32,
        per_session: week.iter().map(|(sid, t)| (sid.clone(), t.cost)).collect(),
        days,
        today_by_session: window_totals(events, today),
    }
}

/// `(name, dollars)` pairs, most expensive first; equal amounts keep their order.
fn ranked(mut costs: Vec<(String, f64)>) -> Vec<(String, f64)> {
    costs.sort_by(|a, b| b.1.total_cmp(&a.1));
    costs
}

fn needs_you(status: &str) -> bool {
    NEEDS_YOU.iter().any(|s| s.as_str() == status)
}

/// Adds up, by hour of the day, the time sessions spent working from `first_day` on.
///
/// A session works from a `working` event to its next status event; work that began the day
/// before counts from midnight. One still working counts up to `now`, [`MAX_OPEN`] at most.
/// Hours and midnight are in the zone `tz`.
///
/// Returns 24 sums of seconds, midnight first.
fn working_hours_in<Tz: TimeZone>(
    events: &Days,
    first_day: NaiveDate,
    now: f64,
    tz: &Tz,
) -> [f64; 24] {
    let mut hours = [0.0; 24];
    let Some(midnight) = midnight_of(first_day, tz) else {
        return hours;
    };
    // In the order sessions started working, which is the order of their sums
    let mut started: Vec<(&str, f64)> = Vec::new();
    let mut spans: Vec<(f64, f64)> = Vec::new();
    let from = first_day.pred_opt().unwrap_or(first_day);
    for e in events.range(from..).flat_map(|(_, evs)| evs.iter()) {
        let Some(status) = e.status.as_deref() else {
            continue;
        };
        if let Some(i) = started.iter().position(|s| s.0 == e.id) {
            spans.push((started.remove(i).1, e.t));
        }
        if status == Status::Working.as_str() {
            started.push((&e.id, e.t));
        }
    }
    spans.extend(started.iter().map(|&(_, t)| (t, now.min(t + MAX_OPEN))));
    for (start, end) in spans {
        let mut start = start.max(midnight);
        while start < end {
            let Some(t) = DateTime::<Utc>::from_timestamp(start.floor() as i64, 0) else {
                break;
            };
            let t = t.with_timezone(tz);
            // The end of the hour `start` falls in, in local time
            let into_hour = f64::from(t.minute() * 60 + t.second());
            let edge = start.floor() - into_hour + 3600.0;
            let stop = end.min(edge);
            hours[t.hour() as usize] += stop - start;
            start = stop;
        }
    }
    hours
}

/// The first moment of a local day, as a Unix timestamp.
fn midnight_of<Tz: TimeZone>(day: NaiveDate, tz: &Tz) -> Option<f64> {
    // A zone that skips midnight starts its day at the first hour that exists
    (0..24)
        .find_map(|h| {
            let time = NaiveTime::from_hms_opt(h, 0, 0)?;
            tz.from_local_datetime(&day.and_time(time)).earliest()
        })
        .map(|t| t.timestamp() as f64)
}

/// Adds up one day of the journal.
///
/// `events` holds each day's events, including the days before. `now` ends a wait still going.
/// `totals` is [`crate::core::journal::window_totals`] of the events from `today` on, if already
/// known.
pub fn day_summary(
    events: &Days,
    today: NaiveDate,
    now: f64,
    totals: Option<&HashMap<String, SessionTotals>>,
) -> Day {
    let mut day = Day::default();
    let mut seen = HashSet::new();
    // Each session's last status and when it began, in the order sessions first show one
    let mut last: Vec<(&str, &str, f64)> = Vec::new();
    for e in events.get(&today).copied().unwrap_or_default() {
        let status = e.status.as_deref();
        if status != Some(ENDED) {
            seen.insert(e.id.as_str());
        }
        let Some(status) = status else { continue };
        match last.iter_mut().find(|l| l.0 == e.id) {
            Some(l) => {
                if needs_you(l.1) {
                    day.waited += e.t - l.2;
                }
                (l.1, l.2) = (status, e.t);
            }
            None => last.push((&e.id, status, e.t)),
        }
    }
    day.waited += last
        .iter()
        .filter(|l| needs_you(l.1))
        .map(|l| (now - l.2).max(0.0))
        .sum::<f64>();
    let owned;
    let totals = match totals {
        Some(t) => t,
        None => {
            owned = window_totals(events, today);
            &owned
        }
    };
    day.added = totals.values().map(|t| t.added).sum();
    day.removed = totals.values().map(|t| t.removed).sum();
    day.sessions = seen.len() as u32;
    day
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::Path;

    use chrono::{Duration, FixedOffset, NaiveDate, TimeZone};
    use tempfile::TempDir;

    use crate::claude::history::HistorySession;
    use crate::core::journal::{Days, JournalEvent, SessionTotals};
    use crate::ui::usage::{day_summary, working_hours_in, Day, Reader, Totals, MAX_OPEN};

    fn tz() -> FixedOffset {
        // Not UTC, so a function that ignores the zone gets the wrong hours and days
        FixedOffset::west_opt(3 * 3600).unwrap()
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 4).unwrap()
    }

    fn ago(days: i64) -> NaiveDate {
        today() - Duration::days(days)
    }

    /// A local time of `day`, as a Unix timestamp.
    fn at(day: NaiveDate, h: u32, m: u32) -> f64 {
        tz().from_local_datetime(&day.and_hms_opt(h, m, 0).unwrap())
            .unwrap()
            .timestamp() as f64
    }

    fn now() -> f64 {
        at(today(), 12, 0)
    }

    fn session(id: &str, cwd: &str) -> HistorySession {
        HistorySession {
            id: id.into(),
            cwd: cwd.into(),
            last: 0.0,
            first_prompt: String::new(),
        }
    }

    fn status(t: f64, id: &str, s: &str) -> JournalEvent {
        JournalEvent {
            t,
            id: id.into(),
            status: Some(s.into()),
            ..Default::default()
        }
    }

    fn usage(t: f64, id: &str, cost: f64, added: u64, removed: u64) -> JournalEvent {
        JournalEvent {
            t,
            id: id.into(),
            cost: Some(cost),
            added: Some(added),
            removed: Some(removed),
            ..Default::default()
        }
    }

    /// Appends an event to the file of the day `days_ago` days back, as the hook does.
    fn log(dir: &Path, days_ago: i64, e: &JournalEvent) {
        let path = dir.join(format!("{}.jsonl", ago(days_ago)));
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .unwrap();
        writeln!(file, "{}", serde_json::to_string(e).unwrap()).unwrap();
    }

    fn cost_log(dir: &Path, id: &str, days_ago: i64, cost: f64, added: u64, removed: u64) {
        let t = now() - days_ago as f64 * 86400.0;
        log(dir, days_ago, &usage(t, id, cost, added, removed));
    }

    fn reader(dir: &TempDir) -> Reader {
        Reader::in_dir(dir.path().to_path_buf())
    }

    #[test]
    fn totals_cover_today_the_week_projects_and_lines() {
        let dir = TempDir::new().unwrap();
        cost_log(dir.path(), "a", 7, 1.0, 5, 1);
        cost_log(dir.path(), "a", 2, 2.0, 10, 2);
        cost_log(dir.path(), "b", 1, 0.5, 0, 0);
        cost_log(dir.path(), "a", 0, 4.0, 25, 4);
        let history = [session("a", "/w/api"), session("b", "/w/web/")];
        let mut reader = reader(&dir);
        let t = reader.totals_in(&history, 0, now(), &tz());
        assert_eq!(t.today, 2.0);
        assert_eq!(
            t.by_project,
            vec![("api".to_string(), 3.0), ("web".to_string(), 0.5)]
        );
        assert_eq!((t.added, t.removed, t.sessions), (20, 3, 2));
        // One amount per day of the week, oldest first, today last
        let days: Vec<_> = (0..7).rev().map(ago).collect();
        assert_eq!(t.days.iter().map(|d| d.0).collect::<Vec<_>>(), days);
        let amounts: Vec<f64> = t.days.iter().map(|d| d.1).collect();
        assert_eq!(amounts, [0.0, 0.0, 0.0, 0.0, 1.0, 0.5, 2.0]);
        // The today line and today's column agree
        assert_eq!(t.today, t.days[6].1);
        let per_session: HashMap<String, f64> = [("a".into(), 3.0), ("b".into(), 0.5)].into();
        assert_eq!(t.per_session, per_session);
    }

    #[test]
    fn totals_name_models_and_skip_sessions_without_a_folder() {
        let dir = TempDir::new().unwrap();
        let mut e = usage(now(), "a", 2.0, 0, 0);
        e.model = Some("opus".into());
        log(dir.path(), 0, &e);
        cost_log(dir.path(), "ghost", 0, 1.0, 0, 0);
        let history = [session("a", "/w/api"), session("empty", "")];
        let t = reader(&dir).totals_in(&history, 0, now(), &tz()).clone();
        assert_eq!(t.by_project, vec![("api".to_string(), 2.0)]);
        assert_eq!(
            t.by_model,
            vec![("opus".to_string(), 2.0), ("unknown".to_string(), 1.0)]
        );
    }

    #[test]
    fn ranked_lists_put_the_most_expensive_first_and_keep_the_order_of_ties() {
        let dir = TempDir::new().unwrap();
        cost_log(dir.path(), "c", 0, 1.0, 0, 0);
        cost_log(dir.path(), "b", 0, 3.0, 0, 0);
        cost_log(dir.path(), "a", 0, 1.0, 0, 0);
        let history = [
            session("a", "/w/first"),
            session("b", "/w/top"),
            session("c", "/w/second"),
        ];
        let t = reader(&dir).totals_in(&history, 0, now(), &tz()).clone();
        // a and c tie: c appeared first in the journal, as in a dict filled in event order
        let names: Vec<&str> = t.by_project.iter().map(|p| p.0.as_str()).collect();
        assert_eq!(names, ["top", "second", "first"]);
    }

    #[test]
    fn totals_are_reused_until_the_journal_or_the_history_changes() {
        let dir = TempDir::new().unwrap();
        cost_log(dir.path(), "a", 0, 1.0, 0, 0);
        let mut reader = reader(&dir);
        let history = [session("a", "/w/api")];
        let first = reader.totals_in(&history, 0, now(), &tz()).clone();
        // The key is what decides, not the history's contents: a stale history proves a reuse
        let other = [session("a", "/w/web")];
        assert_eq!(reader.totals_in(&other, 0, now(), &tz()), &first);
        assert_eq!(
            reader.totals_in(&other, 1, now(), &tz()).by_project,
            vec![("web".to_string(), 1.0)]
        );
        cost_log(dir.path(), "a", 0, 2.0, 0, 0);
        assert_eq!(reader.totals_in(&other, 1, now(), &tz()).today, 2.0);
    }

    #[test]
    fn totals_start_over_on_a_new_day() {
        let dir = TempDir::new().unwrap();
        cost_log(dir.path(), "a", 1, 1.0, 0, 0);
        let mut reader = reader(&dir);
        let history = [session("a", "/w/api")];
        assert_eq!(reader.totals_in(&history, 0, now(), &tz()).today, 0.0);
        let tomorrow = now() + 86400.0;
        let t = reader.totals_in(&history, 0, tomorrow, &tz());
        assert_eq!(t.days.last().map(|d| d.0), Some(ago(-1)));
    }

    #[test]
    fn the_day_and_the_hours_read_what_the_totals_read() {
        let dir = TempDir::new().unwrap();
        cost_log(dir.path(), "a", 1, 1.0, 5, 0);
        cost_log(dir.path(), "a", 0, 2.0, 15, 0);
        log(dir.path(), 0, &status(at(today(), 9, 0), "a", "working"));
        log(dir.path(), 0, &status(at(today(), 9, 30), "a", "done"));
        let mut reader = reader(&dir);
        let history = [session("a", "/w/api")];
        reader.totals_in(&history, 0, now(), &tz());
        let day = reader.day_in(now(), &tz());
        assert_eq!((day.added, day.sessions), (10, 1));
        assert_eq!(reader.hours_in(now(), &tz())[9], 1800.0);
        // The lines come from the totals: a file that grew since is not counted until they run
        cost_log(dir.path(), "a", 0, 3.0, 20, 0);
        assert_eq!(reader.day_in(now() + 1.0, &tz()).added, 10);
        reader.totals_in(&history, 0, now(), &tz());
        assert_eq!(reader.day_in(now(), &tz()).added, 15);
    }

    #[test]
    fn the_day_is_empty_before_the_totals_read_anything() {
        let reader = Reader::in_dir(TempDir::new().unwrap().path().to_path_buf());
        assert_eq!(reader.day_in(now(), &tz()), Day::default());
        assert_eq!(reader.hours_in(now(), &tz()), [0.0; 24]);
    }

    #[test]
    fn day_summary_adds_up_lines_sessions_and_waiting() {
        let yesterday = [usage(at(today(), 0, 0) - 60.0, "a", 1.0, 10, 0)];
        let events = [
            status(at(today(), 9, 0), "a", "input"),
            status(at(today(), 9, 10), "a", "working"),
            usage(at(today(), 9, 20), "a", 2.0, 40, 5),
            status(at(today(), 10, 0), "b", "permission"),
            status(at(today(), 11, 0), "old", "ended"),
        ];
        let days: Days = [(ago(1), &yesterday[..]), (today(), &events[..])].into();
        let day = day_summary(&days, today(), at(today(), 10, 30), None);
        // a waited 10 minutes for an answer; b is still waiting, 30 minutes so far
        assert_eq!(day.waited, 600.0 + 1800.0);
        // Lines count only what the sessions grew by today; an ended session is not one of
        // today's
        assert_eq!((day.added, day.removed, day.sessions), (30, 5, 2));
    }

    #[test]
    fn day_summary_takes_the_totals_it_is_given() {
        let events = [usage(at(today(), 9, 0), "a", 2.0, 40, 5)];
        let days: Days = [(today(), &events[..])].into();
        let given: HashMap<String, SessionTotals> = [(
            "a".to_string(),
            SessionTotals {
                cost: 0.0,
                added: 7,
                removed: 1,
            },
        )]
        .into();
        let day = day_summary(&days, today(), now(), Some(&given));
        assert_eq!((day.added, day.removed), (7, 1));
    }

    #[test]
    fn working_hours_split_spans_across_hours() {
        let day = ago(1);
        let nine = at(day, 9, 30);
        let events = [
            status(nine, "a", "working"),
            status(nine + 3600.0, "a", "done"),
            status(nine, "b", "permission"),
            status(nine + 600.0, "b", "working"),
        ];
        let days: Days = [(day, &events[..])].into();
        let hours = working_hours_in(&days, day, nine + 1200.0, &tz());
        // a: 9:30-10:30; b: 9:40 until now, 9:50
        assert_eq!((hours[9], hours[10]), (1800.0 + 600.0, 1800.0));
        assert_eq!(hours.iter().sum::<f64>(), 4200.0);
        // A session left working with no end counts two hours at most
        let late = working_hours_in(&days, day, nine + 86400.0, &tz());
        assert_eq!(late.iter().sum::<f64>(), 3600.0 + MAX_OPEN);
    }

    #[test]
    fn working_hours_count_work_from_yesterday_from_midnight() {
        let midnight = at(today(), 0, 0);
        let before = [status(midnight - 600.0, "a", "working")];
        let after = [status(midnight + 5.0 * 3600.0, "a", "done")];
        let days: Days = [(ago(1), &before[..]), (today(), &after[..])].into();
        let hours = working_hours_in(&days, today(), midnight + 6.0 * 3600.0, &tz());
        // A closed span counts whole, however long
        assert_eq!(hours[..5], [3600.0; 5]);
        assert_eq!(hours.iter().sum::<f64>(), 5.0 * 3600.0);
    }

    #[test]
    fn working_hours_follow_a_zone_that_is_not_on_the_hour() {
        let zone = FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
        let start = zone
            .from_local_datetime(&today().and_hms_opt(9, 45, 0).unwrap())
            .unwrap()
            .timestamp() as f64;
        let events = [
            status(start, "a", "working"),
            status(start + 1800.0, "a", "done"),
        ];
        let days: Days = [(today(), &events[..])].into();
        let hours = working_hours_in(&days, today(), start + 3600.0, &zone);
        assert_eq!((hours[9], hours[10]), (900.0, 900.0));
    }

    #[test]
    fn totals_and_day_load_from_a_subset_of_fields() {
        let t: Totals =
            serde_json::from_str(r#"{"today": 1.5, "by_project": [["x", 2]]}"#).unwrap();
        assert_eq!((t.today, t.by_project), (1.5, vec![("x".to_string(), 2.0)]));
        let d: Day = serde_json::from_str(r#"{"added": 3, "waited": 720}"#).unwrap();
        assert_eq!((d.added, d.waited, d.sessions), (3, 720.0, 0));
    }
}
