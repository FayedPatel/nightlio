//! Port of `api/database_achievements.py` (`AchievementsMixin`): stats,
//! streak, and achievement utilities. Owned by the achievements data-mixin
//! agent — no other agent edits this file.
//!
//! Semantics preserved from the Python side (bug-compatible per
//! `contract/DECISIONS.md`):
//! - `first_entry_date` / `last_entry_date` are lexicographic MIN/MAX over
//!   the raw stored date strings (DECISIONS.md #4) — a US-format `8/2/2025`
//!   row sorts after every ISO date, and even a `not-a-date` row can win.
//! - `average_mood` is `round(x, 2) if x else 0`: a float when entries
//!   exist, the **integer** `0` otherwise (`AverageMood` serializes both
//!   shapes exactly).
//! - `check_achievements` returns bare `achievement_type` strings
//!   (DECISIONS.md #8); metadata merging happens in the route layer.
//! - The streak calculator parses both `%m/%d/%Y` and `%Y-%m-%d` (first
//!   match wins, in that order), silently skips unparseable rows, dedupes
//!   equal calendar days stored under different raw strings, and swallows
//!   every error into a streak of 0 — exactly like the Python.
//! - contract change (owner-approved, supersedes DECISIONS.md #9):
//!   statistics views are recorded explicitly and per-day idempotent —
//!   `record_stats_view` increments `stats_views` at most once per calendar
//!   day (tracked in `user_metrics.last_view_date`). The `data_lover`
//!   achievement still unlocks at `stats_views >= 10`, now meaning "viewed
//!   statistics on 10 different days".
//!
//! All functions take `&Connection`; callers run them under
//! `tokio::task::spawn_blocking`.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::common::{DatabaseError, MoodValue, sql_queries};

// --- Achievements-specific SQL (verbatim from database_achievements.py) -----
//
// The triple-quoted Python literals are copied byte-for-byte, including the
// leading newline and interior indentation.

/// contract change: the UPSERT increments `stats_views` only when the
/// stored `last_view_date` is NULL or differs from today's ISO date, and
/// always pins `last_view_date` to today (the boolean subexpression
/// evaluates to 0/1 in SQLite).
const RECORD_STATS_VIEW_SQL: &str = "
                INSERT INTO user_metrics (user_id, stats_views, last_view_date)
                VALUES (?1, 1, ?2)
                ON CONFLICT(user_id)
                DO UPDATE SET stats_views = stats_views
                                  + (last_view_date IS NULL OR last_view_date <> ?2),
                              last_view_date = ?2,
                              updated_at = CURRENT_TIMESTAMP
                ";

const GET_USER_METRICS_SQL: &str =
    "SELECT user_id, stats_views, updated_at FROM user_metrics WHERE user_id = ?";

const GET_MOOD_COUNTS_SQL: &str = "
                SELECT mood, COUNT(*) as count
                  FROM mood_entries
                 WHERE user_id = ?
                 GROUP BY mood
                 ORDER BY mood
                ";

const ADD_ACHIEVEMENT_SQL: &str =
    "INSERT INTO achievements (user_id, achievement_type) VALUES (?, ?)";

const GET_USER_ACHIEVEMENTS_SQL: &str = "
                SELECT id, achievement_type, earned_at, nft_minted, nft_token_id, nft_tx_hash
                  FROM achievements
                 WHERE user_id = ?
                 ORDER BY earned_at DESC
                ";

const UPDATE_ACHIEVEMENT_NFT_SQL: &str = "
                UPDATE achievements
                   SET nft_minted = TRUE,
                       nft_token_id = ?,
                       nft_tx_hash = ?
                 WHERE id = ?
                ";

// --- Row structs -------------------------------------------------------------

/// `average_mood`: Python emits `round(x, 2)` (a JSON float) when the
/// average is truthy, and the bare integer `0` otherwise. An untagged enum
/// keeps both JSON shapes distinct (`0` vs `0.0`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(untagged)]
pub enum AverageMood {
    Int(i64),
    Float(f64),
}

/// Result of `get_mood_statistics` — the `statistics` object inside
/// `GET /api/statistics` (see `contract/fixtures/mood/statistics_*.json`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MoodStatistics {
    pub total_entries: i64,
    pub average_mood: AverageMood,
    /// MIN/MAX over the stored mood values, read tolerantly ([`MoodValue`]):
    /// a schema-legal REAL row (e.g. `4.5`, which passes the 1..=5 CHECK)
    /// must serve like Python's dynamic typing instead of 500ing.
    pub lowest_mood: Option<MoodValue>,
    pub highest_mood: Option<MoodValue>,
    pub first_entry_date: Option<String>,
    pub last_entry_date: Option<String>,
}

/// Result of `get_user_metrics`. Never serialized to the wire by Flask
/// (only consumed internally by `check_achievements` /
/// `get_achievements_progress`), but kept `Serialize` for symmetry. When no
/// row exists the Python returns `{"user_id": ..., "stats_views": 0}` with
/// no `updated_at` key — modeled with `skip_serializing_if`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UserMetrics {
    pub user_id: i64,
    pub stats_views: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// One row of `get_user_achievements` — the DB-row half of the objects in
/// `contract/fixtures/achievements/get_achievements_unlocked.json` (the
/// route layer merges in name/description/icon/rarity metadata).
/// `nft_minted` is the raw SQLite integer (0/1), matching the recorded
/// `"nft_minted": 0`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AchievementRow {
    pub id: i64,
    pub achievement_type: String,
    pub earned_at: Option<String>,
    pub nft_minted: Option<i64>,
    pub nft_token_id: Option<i64>,
    pub nft_tx_hash: Option<String>,
}

/// One `{current, max}` pair in the progress map.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ProgressEntry {
    pub current: i64,
    pub max: i64,
}

/// Result of `get_achievements_progress`
/// (`contract/fixtures/achievements/get_achievements_progress_seeded.json`).
/// Fields are declared in Flask's emitted (alphabetical, `jsonify`
/// sort_keys) order.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AchievementsProgress {
    pub consistency_king: ProgressEntry,
    pub data_lover: ProgressEntry,
    pub first_entry: ProgressEntry,
    pub mood_master: ProgressEntry,
    pub week_warrior: ProgressEntry,
}

// --- Statistics --------------------------------------------------------------

/// contract change (owner-approved, supersedes DECISIONS.md #9 and
/// the pre-rewrite unconditional `increment_stats_view` counter bump): record a statistics view for the
/// server-local calendar day. Returns `counted` — whether this call
/// actually incremented `stats_views` (false when today was already
/// recorded). Backs `POST /api/statistics/view`; `GET /api/statistics` is
/// now a pure read.
pub fn record_stats_view(conn: &Connection, user_id: i64) -> Result<bool, DatabaseError> {
    record_stats_view_on(conn, user_id, chrono::Local::now().date_naive())
}

/// Day-parameterized core of [`record_stats_view`] (also used by tests and
/// the achievements-check seeding path). Increments `stats_views` only when
/// `last_view_date` is NULL or differs from `today`; always sets
/// `last_view_date = today` and bumps `updated_at`.
pub fn record_stats_view_on(
    conn: &Connection,
    user_id: i64,
    today: NaiveDate,
) -> Result<bool, DatabaseError> {
    let today = today.format("%Y-%m-%d").to_string();
    let prior: Option<Option<String>> = conn
        .query_row(
            "SELECT last_view_date FROM user_metrics WHERE user_id = ?",
            [user_id],
            |row| row.get(0),
        )
        .optional()?;
    let counted = prior.flatten().as_deref() != Some(today.as_str());
    conn.execute(RECORD_STATS_VIEW_SQL, rusqlite::params![user_id, today])?;
    Ok(counted)
}

/// Port of `get_user_metrics`. A NULL `stats_views` is coerced to 0 —
/// every Python use site does `int(... or 0)`.
pub fn get_user_metrics(conn: &Connection, user_id: i64) -> Result<UserMetrics, DatabaseError> {
    let row = conn
        .query_row(GET_USER_METRICS_SQL, [user_id], |row| {
            Ok(UserMetrics {
                user_id: row.get(0)?,
                stats_views: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                updated_at: row.get(2)?,
            })
        })
        .optional()?;
    Ok(row.unwrap_or(UserMetrics {
        user_id,
        stats_views: 0,
        updated_at: None,
    }))
}

/// Port of `get_mood_statistics` — verbatim `SQLQueries.GET_MOOD_STATISTICS`
/// (lexicographic MIN/MAX over raw date strings; `average_mood` is
/// `round(x, 2) if x else 0`).
pub fn get_mood_statistics(
    conn: &Connection,
    user_id: i64,
) -> Result<MoodStatistics, DatabaseError> {
    let row = conn.query_row(sql_queries::GET_MOOD_STATISTICS, [user_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<f64>>(1)?,
            row.get::<_, Option<MoodValue>>(2)?,
            row.get::<_, Option<MoodValue>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    if row.0 > 0 {
        // Python: `round(row[1], 2) if row[1] else 0` — falsy (None or 0.0)
        // collapses to the integer 0.
        let average_mood = match row.1 {
            Some(x) if x != 0.0 => AverageMood::Float(python_round2(x)),
            _ => AverageMood::Int(0),
        };
        Ok(MoodStatistics {
            total_entries: row.0,
            average_mood,
            lowest_mood: row.2,
            highest_mood: row.3,
            first_entry_date: row.4,
            last_entry_date: row.5,
        })
    } else {
        Ok(MoodStatistics {
            total_entries: 0,
            average_mood: AverageMood::Int(0),
            lowest_mood: None,
            highest_mood: None,
            first_entry_date: None,
            last_entry_date: None,
        })
    }
}

/// Python's `round(x, 2)`: correctly rounded to 2 decimal places with ties
/// to even. Rust's `{:.2}` formatting performs the same correctly-rounded,
/// ties-to-even conversion on the exact binary value, so a format/parse
/// round trip reproduces it (e.g. `3.125 -> 3.12`, `2.675 -> 2.67`).
fn python_round2(x: f64) -> f64 {
    format!("{x:.2}").parse().unwrap_or(x)
}

/// Port of `get_mood_counts`. Python builds a `{mood: count}` dict and
/// Flask's `jsonify` (sort_keys) emits it with sorted string keys, e.g.
/// `{"2": 1, "4.5": 1}`. The mood key is read tolerantly ([`MoodValue`]) —
/// SQLite's dynamic typing admits schema-legal REAL moods like `4.5` — and
/// stored under its Python string form in a `BTreeMap`, whose lexicographic
/// order matches both Flask's sorted output and numeric order for every
/// CHECK-legal mood in `[1, 5]` (single integer digit, so string comparison
/// of the fraction equals numeric comparison).
pub fn get_mood_counts(
    conn: &Connection,
    user_id: i64,
) -> Result<BTreeMap<String, i64>, DatabaseError> {
    let mut stmt = conn.prepare(GET_MOOD_COUNTS_SQL)?;
    let counts = stmt
        .query_map([user_id], |row| {
            Ok((
                row.get::<_, MoodValue>(0)?.python_key_string(),
                row.get::<_, i64>(1)?,
            ))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    Ok(counts)
}

// --- Streak calculation -------------------------------------------------------

/// Port of `get_current_streak`: any error (SQL or otherwise) is swallowed
/// into 0 with a warning, exactly like the Python `except Exception` guard.
pub fn get_current_streak(conn: &Connection, user_id: i64) -> i64 {
    match get_user_entry_dates(conn, user_id) {
        Ok(dates) => {
            if dates.is_empty() {
                return 0;
            }
            let parsed = parse_date_strings(&dates);
            if parsed.is_empty() {
                return 0;
            }
            calculate_streak_from_dates(&parsed, chrono::Local::now().date_naive())
        }
        Err(exc) => {
            tracing::warn!("Error calculating streak for user {user_id}: {exc}");
            0
        }
    }
}

/// Port of `_get_user_entry_dates` — verbatim
/// `SQLQueries.GET_USER_ENTRY_DATES` (DISTINCT on the raw string).
fn get_user_entry_dates(conn: &Connection, user_id: i64) -> Result<Vec<String>, DatabaseError> {
    let mut stmt = conn.prepare(sql_queries::GET_USER_ENTRY_DATES)?;
    let dates = stmt
        .query_map([user_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(dates)
}

/// Port of `_parse_date_strings`: try `%m/%d/%Y` then `%Y-%m-%d`, skip
/// unparseable strings, dedupe (the SQL DISTINCT is on the raw string, so
/// the same day stored once as ISO and once as M/D/YYYY parses to two
/// equal dates), and sort descending.
fn parse_date_strings(date_strings: &[String]) -> Vec<NaiveDate> {
    let mut parsed: std::collections::BTreeSet<NaiveDate> = std::collections::BTreeSet::new();
    for date_str in date_strings {
        if let Some(date) = parse_python_date(date_str) {
            parsed.insert(date);
        } else {
            tracing::debug!("Could not parse date: {date_str}");
        }
    }
    parsed.into_iter().rev().collect()
}

/// `datetime.strptime(s, fmt)` equivalent for the two formats the Python
/// tries, replicating CPython's `_strptime` regexes exactly:
/// `%Y` is exactly four digits, `%m`/`%d` are one or two digits with no
/// extra padding, and impossible calendar dates (e.g. `4/31/2025`,
/// `2/29/2025`) raise/skip. Format order matters: `%m/%d/%Y` first.
fn parse_python_date(s: &str) -> Option<NaiveDate> {
    parse_with_separator(s, '/', false).or_else(|| parse_with_separator(s, '-', true))
}

fn parse_with_separator(s: &str, sep: char, year_first: bool) -> Option<NaiveDate> {
    let mut parts = s.split(sep);
    let (a, b, c) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let (year_str, month_str, day_str) = if year_first { (a, b, c) } else { (c, a, b) };
    // CPython's TimeRE: %Y is exactly \d{4}; %m and %d are 1-2 digits.
    if year_str.len() != 4 || !year_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let field = |t: &str| -> Option<u32> {
        if t.is_empty() || t.len() > 2 || !t.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        t.parse().ok()
    };
    let (month, day) = (field(month_str)?, field(day_str)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year: i32 = year_str.parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

/// Port of `_calculate_streak_from_dates`. `parsed_dates` must be sorted
/// descending and deduplicated. A most-recent entry more than one day in
/// the past kills the streak; entries dated in the future do not (the
/// Python only checks `(today - most_recent).days > 1`).
fn calculate_streak_from_dates(parsed_dates: &[NaiveDate], today: NaiveDate) -> i64 {
    let Some(&most_recent) = parsed_dates.first() else {
        return 0;
    };
    if (today - most_recent).num_days() > 1 {
        return 0;
    }
    let mut streak = 0;
    let mut expected_date = most_recent;
    for &entry_date in parsed_dates {
        if entry_date == expected_date {
            streak += 1;
            expected_date = entry_date - chrono::Days::new(1);
        } else {
            break;
        }
    }
    streak
}

// --- Achievements -------------------------------------------------------------

/// Port of `add_achievement`: `None` on the UNIQUE(user_id,
/// achievement_type) violation (the Python catches only
/// `sqlite3.IntegrityError`); other errors propagate.
pub fn add_achievement(
    conn: &Connection,
    user_id: i64,
    achievement_type: &str,
) -> Result<Option<i64>, DatabaseError> {
    match conn.execute(
        ADD_ACHIEVEMENT_SQL,
        rusqlite::params![user_id, achievement_type],
    ) {
        Ok(_) => Ok(Some(conn.last_insert_rowid())),
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Ok(None)
        }
        Err(exc) => Err(exc.into()),
    }
}

/// Port of `get_user_achievements` — rows ordered by `earned_at DESC`.
pub fn get_user_achievements(
    conn: &Connection,
    user_id: i64,
) -> Result<Vec<AchievementRow>, DatabaseError> {
    let mut stmt = conn.prepare(GET_USER_ACHIEVEMENTS_SQL)?;
    let rows = stmt
        .query_map([user_id], |row| {
            Ok(AchievementRow {
                id: row.get(0)?,
                achievement_type: row.get(1)?,
                earned_at: row.get(2)?,
                nft_minted: row.get(3)?,
                nft_token_id: row.get(4)?,
                nft_tx_hash: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Port of `update_achievement_nft`.
pub fn update_achievement_nft(
    conn: &Connection,
    achievement_id: i64,
    token_id: i64,
    tx_hash: &str,
) -> Result<(), DatabaseError> {
    conn.execute(
        UPDATE_ACHIEVEMENT_NFT_SQL,
        rusqlite::params![token_id, tx_hash, achievement_id],
    )?;
    Ok(())
}

/// Port of `check_achievements`: awards every newly-met achievement and
/// returns the bare `achievement_type` strings, in the fixed check order
/// (DECISIONS.md #8 — this is the `POST /api/mood` `new_achievements`
/// shape; routes wanting metadata objects merge it themselves).
pub fn check_achievements(conn: &Connection, user_id: i64) -> Result<Vec<String>, DatabaseError> {
    let total_entries = get_mood_statistics(conn, user_id)?.total_entries;
    let current_streak = get_current_streak(conn, user_id);
    let stats_views = get_user_metrics(conn, user_id)?.stats_views;

    let achievements_to_check = [
        ("first_entry", total_entries >= 1),
        ("week_warrior", current_streak >= 7),
        ("consistency_king", current_streak >= 30),
        ("data_lover", stats_views >= 10),
        ("mood_master", total_entries >= 100),
    ];

    let mut new_achievements = Vec::new();
    for (achievement_type, condition) in achievements_to_check {
        if condition {
            // Python: `if achievement_id:` — both None and 0 are falsy.
            if let Some(id) = add_achievement(conn, user_id, achievement_type)?
                && id != 0
            {
                new_achievements.push(achievement_type.to_string());
            }
        }
    }
    Ok(new_achievements)
}

/// Port of `get_achievements_progress` — `current` clamped to `[0, max]`.
pub fn get_achievements_progress(
    conn: &Connection,
    user_id: i64,
) -> Result<AchievementsProgress, DatabaseError> {
    let total_entries = get_mood_statistics(conn, user_id)?.total_entries;
    let current_streak = get_current_streak(conn, user_id);
    let stats_views = get_user_metrics(conn, user_id)?.stats_views;

    let entry = |value: i64, maximum: i64| ProgressEntry {
        current: value.clamp(0, maximum),
        max: maximum,
    };

    Ok(AchievementsProgress {
        consistency_king: entry(current_streak, 30),
        data_lover: entry(stats_views, 10),
        first_entry: entry(total_entries, 1),
        mood_master: entry(total_entries, 100),
        week_warrior: entry(current_streak, 7),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Days, Local};
    use serde_json::json;

    /// Fresh bootstrapped DB (default self-host user is id 1).
    fn test_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("achievements-test.db");
        let path = path.to_str().unwrap();
        crate::db::bootstrap(path, &crate::db::SelfHostSeed::default()).unwrap();
        (dir, crate::db::connect(path).unwrap())
    }

    fn create_user(conn: &Connection, google_id: &str) -> i64 {
        conn.execute(
            "INSERT INTO users (google_id, email, name) VALUES (?, ?, ?)",
            rusqlite::params![google_id, format!("{google_id}@x"), google_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn add_entry(conn: &Connection, user_id: i64, date: &str, mood: i64) {
        conn.execute(
            "INSERT INTO mood_entries (user_id, date, mood, content) VALUES (?, ?, ?, 'x')",
            rusqlite::params![user_id, date, mood],
        )
        .unwrap();
    }

    /// Seed identical to user A in the Python cross-check script
    /// (scratchpad `xcheck.py`, run against `api/venv/bin/python` +
    /// `api/database.py` on 2026-08-15). Fixed absolute dates only, so the
    /// pinned Python outputs are stable across test-run dates.
    fn seed_user_a(conn: &Connection) -> i64 {
        let ua = create_user(conn, "userA");
        for (d, m) in [
            ("2025-08-01", 2),
            ("2025-12-31", 4),
            ("8/2/2025", 5),
            ("1/5/2025", 1),
            ("2025-03-15", 3),
            ("3/15/2025", 5), // same calendar day as previous, different raw string
            ("2025-01-02", 4),
            ("not-a-date", 1), // silently excluded by the streak parser
        ] {
            add_entry(conn, ua, d, m);
        }
        ua
    }

    fn us_format(d: NaiveDate) -> String {
        // Python xcheck seeds f"{d.month}/{d.day}/{d.year}" — no padding.
        d.format("%-m/%-d/%Y").to_string()
    }

    /// Seed identical to user B in the Python cross-check: today (ISO and
    /// US duplicate), yesterday ISO, two days ago US. Relative dates, so
    /// the pinned streak of 3 holds on any run date.
    fn seed_user_b(conn: &Connection) -> (i64, Vec<String>) {
        let ub = create_user(conn, "userB");
        let today = Local::now().date_naive();
        let dates = vec![
            today.format("%Y-%m-%d").to_string(),
            us_format(today),
            (today - Days::new(1)).format("%Y-%m-%d").to_string(),
            us_format(today - Days::new(2)),
        ];
        for (d, m) in dates.iter().zip([4, 5, 3, 2]) {
            add_entry(conn, ub, d, m);
        }
        (ub, dates)
    }

    // --- statistics: values pinned from the Python side (xcheck.py) ---------

    #[test]
    fn user_a_statistics_match_python() {
        let (_dir, conn) = test_db();
        let ua = seed_user_a(&conn);
        let stats = get_mood_statistics(&conn, ua).unwrap();
        // Python: {"total_entries": 8, "average_mood": 3.12, "lowest_mood": 1,
        //          "highest_mood": 5, "first_entry_date": "1/5/2025",
        //          "last_entry_date": "not-a-date"}
        // 25/8 = 3.125 -> round(3.125, 2) = 3.12 (ties to even); MIN/MAX are
        // lexicographic over raw strings, so "not-a-date" ('n' > any digit)
        // wins MAX — the DECISIONS.md #4 quirk.
        assert_eq!(
            serde_json::to_value(&stats).unwrap(),
            json!({
                "total_entries": 8,
                "average_mood": 3.12,
                "lowest_mood": 1,
                "highest_mood": 5,
                "first_entry_date": "1/5/2025",
                "last_entry_date": "not-a-date"
            })
        );
        assert_eq!(stats.average_mood, AverageMood::Float(3.12));
    }

    #[test]
    fn user_a_mood_counts_match_python() {
        let (_dir, conn) = test_db();
        let ua = seed_user_a(&conn);
        let counts = get_mood_counts(&conn, ua).unwrap();
        // Python: {1: 2, 2: 1, 3: 1, 4: 2, 5: 2}; JSON keys are strings.
        assert_eq!(
            serde_json::to_value(&counts).unwrap(),
            json!({"1": 2, "2": 1, "3": 1, "4": 2, "5": 2})
        );
    }

    #[test]
    fn schema_legal_real_mood_statistics_match_python() {
        let (_dir, conn) = test_db();
        let user = create_user(&conn, "userReal");
        // SQLite dynamic typing: 4.5 passes the 1..=5 CHECK and stays REAL
        // under INTEGER affinity. Previously these reads 500ed with
        // InvalidColumnType (mood/lowest/highest as i64).
        conn.execute(
            "INSERT INTO mood_entries (user_id, date, mood, content) VALUES (?, '2025-08-02', 4.5, 'x')",
            [user],
        )
        .unwrap();
        for (d, m) in [("2025-08-01", 2), ("2025-08-03", 4)] {
            add_entry(&conn, user, d, m);
        }

        // Pinned from Python (scratchpad real_mood_xcheck.py, 2026-08-15):
        // statistics: {"average_mood": 3.5, "highest_mood": 4.5,
        //              "lowest_mood": 2, "total_entries": 3, ...}
        let stats = get_mood_statistics(&conn, user).unwrap();
        assert_eq!(
            serde_json::to_value(&stats).unwrap(),
            json!({
                "total_entries": 3,
                "average_mood": 3.5,
                "lowest_mood": 2,
                "highest_mood": 4.5,
                "first_entry_date": "2025-08-01",
                "last_entry_date": "2025-08-03"
            })
        );
        assert_eq!(stats.highest_mood, Some(MoodValue::Float(4.5)));

        // Pinned from Python: json.dumps(counts, sort_keys=True) ==
        // {"2": 1, "4": 1, "4.5": 1} — the float key becomes "4.5".
        let counts = get_mood_counts(&conn, user).unwrap();
        assert_eq!(
            serde_json::to_value(&counts).unwrap(),
            json!({"2": 1, "4": 1, "4.5": 1})
        );
    }

    #[test]
    fn user_a_streak_is_zero_and_check_awards_first_entry_once() {
        let (_dir, conn) = test_db();
        let ua = seed_user_a(&conn);
        // Python: streak 0 (most recent parseable date long past).
        assert_eq!(get_current_streak(&conn, ua), 0);
        // Python: check -> ["first_entry"], second check -> [].
        assert_eq!(check_achievements(&conn, ua).unwrap(), vec!["first_entry"]);
        assert_eq!(check_achievements(&conn, ua).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn user_a_progress_matches_python() {
        let (_dir, conn) = test_db();
        let ua = seed_user_a(&conn);
        let progress = get_achievements_progress(&conn, ua).unwrap();
        // Python: first_entry 1/1, week_warrior 0/7, consistency_king 0/30,
        //         data_lover 0/10, mood_master 8/100.
        assert_eq!(
            serde_json::to_value(progress).unwrap(),
            json!({
                "consistency_king": {"current": 0, "max": 30},
                "data_lover": {"current": 0, "max": 10},
                "first_entry": {"current": 1, "max": 1},
                "mood_master": {"current": 8, "max": 100},
                "week_warrior": {"current": 0, "max": 7}
            })
        );
    }

    #[test]
    fn user_b_streak_statistics_and_check_match_python() {
        let (_dir, conn) = test_db();
        let (ub, dates) = seed_user_b(&conn);

        // Python: streak 3 — today + US-format duplicate of today dedupe to
        // one day; today/yesterday/two-days-ago are consecutive.
        assert_eq!(get_current_streak(&conn, ub), 3);

        // Python: {"total_entries": 4, "average_mood": 3.5, "lowest_mood": 2,
        //          "highest_mood": 5}; entry dates are lexicographic MIN/MAX
        //          of the raw seeded strings (run-date dependent, so computed
        //          here the same way SQLite's memcmp collation does).
        let stats = get_mood_statistics(&conn, ub).unwrap();
        assert_eq!(stats.total_entries, 4);
        assert_eq!(stats.average_mood, AverageMood::Float(3.5));
        assert_eq!(stats.lowest_mood, Some(MoodValue::Int(2)));
        assert_eq!(stats.highest_mood, Some(MoodValue::Int(5)));
        assert_eq!(
            stats.first_entry_date.as_deref(),
            dates.iter().min().map(|s| s.as_str())
        );
        assert_eq!(
            stats.last_entry_date.as_deref(),
            dates.iter().max().map(|s| s.as_str())
        );

        // contract change: 10 recorded views on 10 DISTINCT days reach
        // metrics {stats_views: 10} and check -> ["first_entry",
        // "data_lover"] (check-list order). Each distinct day counts.
        let start = d("2026-07-01");
        for offset in 0..10 {
            assert!(
                record_stats_view_on(&conn, ub, start + Days::new(offset)).unwrap(),
                "each new day must count"
            );
        }
        let metrics = get_user_metrics(&conn, ub).unwrap();
        assert_eq!(metrics.user_id, ub);
        assert_eq!(metrics.stats_views, 10);
        assert!(metrics.updated_at.is_some());
        assert_eq!(
            check_achievements(&conn, ub).unwrap(),
            vec!["first_entry", "data_lover"]
        );

        // Python progress: first_entry 1/1, week_warrior 3/7,
        // consistency_king 3/30, data_lover 10/10, mood_master 4/100.
        assert_eq!(
            serde_json::to_value(get_achievements_progress(&conn, ub).unwrap()).unwrap(),
            json!({
                "consistency_king": {"current": 3, "max": 30},
                "data_lover": {"current": 10, "max": 10},
                "first_entry": {"current": 1, "max": 1},
                "mood_master": {"current": 4, "max": 100},
                "week_warrior": {"current": 3, "max": 7}
            })
        );
    }

    #[test]
    fn empty_user_matches_python_shapes() {
        let (_dir, conn) = test_db();
        // Python empty statistics: average_mood is the INTEGER 0.
        let stats = get_mood_statistics(&conn, 999).unwrap();
        let value = serde_json::to_value(&stats).unwrap();
        assert_eq!(
            value,
            json!({
                "total_entries": 0,
                "average_mood": 0,
                "lowest_mood": null,
                "highest_mood": null,
                "first_entry_date": null,
                "last_entry_date": null
            })
        );
        assert!(
            value["average_mood"].is_i64(),
            "empty average_mood must serialize as integer 0, not 0.0"
        );

        assert!(get_mood_counts(&conn, 999).unwrap().is_empty());
        assert_eq!(get_current_streak(&conn, 999), 0);

        // Python: {"user_id": 999, "stats_views": 0} — no updated_at key.
        let metrics = get_user_metrics(&conn, 999).unwrap();
        assert_eq!(
            serde_json::to_value(&metrics).unwrap(),
            json!({"user_id": 999, "stats_views": 0})
        );
    }

    // --- achievement CRUD ----------------------------------------------------

    #[test]
    fn add_achievement_returns_none_on_duplicate() {
        let (_dir, conn) = test_db();
        let first = add_achievement(&conn, 1, "first_entry").unwrap();
        assert!(matches!(first, Some(id) if id > 0));
        // UNIQUE(user_id, achievement_type) -> IntegrityError -> None.
        assert_eq!(add_achievement(&conn, 1, "first_entry").unwrap(), None);
        // Same type for another user is fine.
        let ua = create_user(&conn, "userA");
        assert!(add_achievement(&conn, ua, "first_entry").unwrap().is_some());
    }

    #[test]
    fn get_user_achievements_orders_by_earned_at_desc_with_db_row_shape() {
        let (_dir, conn) = test_db();
        for (t, earned) in [
            ("first_entry", "2025-01-01 00:00:00"),
            ("week_warrior", "2025-03-01 00:00:00"),
            ("data_lover", "2025-02-01 00:00:00"),
        ] {
            conn.execute(
                "INSERT INTO achievements (user_id, achievement_type, earned_at) VALUES (1, ?, ?)",
                rusqlite::params![t, earned],
            )
            .unwrap();
        }
        let rows = get_user_achievements(&conn, 1).unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| r.achievement_type.as_str())
                .collect::<Vec<_>>(),
            vec!["week_warrior", "data_lover", "first_entry"]
        );
        // DB-row half of get_achievements_unlocked.json: nft_minted is the
        // raw integer 0, token id / tx hash null.
        assert_eq!(
            serde_json::to_value(&rows[0]).unwrap(),
            json!({
                "id": 2,
                "achievement_type": "week_warrior",
                "earned_at": "2025-03-01 00:00:00",
                "nft_minted": 0,
                "nft_token_id": null,
                "nft_tx_hash": null
            })
        );
        // Other users see nothing.
        assert!(get_user_achievements(&conn, 42).unwrap().is_empty());
    }

    #[test]
    fn update_achievement_nft_sets_minted_flag_and_metadata() {
        let (_dir, conn) = test_db();
        let id = add_achievement(&conn, 1, "first_entry").unwrap().unwrap();
        update_achievement_nft(&conn, id, 7, "0xabc").unwrap();
        let row = &get_user_achievements(&conn, 1).unwrap()[0];
        assert_eq!(row.nft_minted, Some(1));
        assert_eq!(row.nft_token_id, Some(7));
        assert_eq!(row.nft_tx_hash.as_deref(), Some("0xabc"));
    }

    #[test]
    fn record_stats_view_counts_once_per_day() {
        // contract change: per-day idempotent view recording.
        let (_dir, conn) = test_db();
        // Fixed days safely in the past so the wall-clock assertions at the
        // end can never collide with the test run date.
        let day_one = d("2020-01-01");

        // First view of the day upserts the row and counts.
        assert!(record_stats_view_on(&conn, 1, day_one).unwrap());
        assert_eq!(get_user_metrics(&conn, 1).unwrap().stats_views, 1);

        // Second view the same day does NOT count.
        assert!(!record_stats_view_on(&conn, 1, day_one).unwrap());
        assert_eq!(get_user_metrics(&conn, 1).unwrap().stats_views, 1);

        // The next day counts again.
        assert!(record_stats_view_on(&conn, 1, day_one + Days::new(1)).unwrap());
        let metrics = get_user_metrics(&conn, 1).unwrap();
        assert_eq!(metrics.stats_views, 2);
        assert!(metrics.updated_at.is_some());

        // The wall-clock wrapper agrees with the _on core for "today".
        let today = Local::now().date_naive();
        assert!(record_stats_view(&conn, 1).unwrap());
        assert!(!record_stats_view_on(&conn, 1, today).unwrap());
        assert_eq!(get_user_metrics(&conn, 1).unwrap().stats_views, 3);
    }

    // --- streak calculator unit tests (fixed `today`) ------------------------

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn streak_counts_consecutive_days_ending_today() {
        let today = d("2026-08-15");
        let dates = [
            d("2026-08-15"),
            d("2026-08-14"),
            d("2026-08-13"),
            d("2026-08-10"),
        ];
        assert_eq!(calculate_streak_from_dates(&dates, today), 3);
    }

    #[test]
    fn streak_may_end_yesterday_but_not_two_days_ago() {
        let today = d("2026-08-15");
        assert_eq!(
            calculate_streak_from_dates(&[d("2026-08-14"), d("2026-08-13")], today),
            2
        );
        assert_eq!(
            calculate_streak_from_dates(&[d("2026-08-13"), d("2026-08-12")], today),
            0
        );
    }

    #[test]
    fn streak_tolerates_future_most_recent_like_python() {
        // Python only checks `(today - most_recent).days > 1`; a future
        // most-recent date passes and seeds the walk.
        let today = d("2026-08-15");
        assert_eq!(
            calculate_streak_from_dates(&[d("2026-08-20"), d("2026-08-19")], today),
            2
        );
    }

    #[test]
    fn parse_date_strings_dedupes_and_skips_unparseable() {
        let raw = [
            "2026-08-15".to_string(),
            "8/15/2026".to_string(), // same day, different raw string -> dedup
            "8/14/2026".to_string(),
            "not-a-date".to_string(), // skipped
            "2/29/2025".to_string(),  // invalid calendar date -> skipped
            "2/29/2024".to_string(),  // leap day -> kept
            "4/31/2025".to_string(),  // April 31 -> skipped
            "025-01-01".to_string(),  // %Y must be exactly 4 digits -> skipped
        ];
        let parsed = parse_date_strings(&raw);
        assert_eq!(
            parsed,
            vec![d("2026-08-15"), d("2026-08-14"), d("2024-02-29")]
        );
    }

    #[test]
    fn parse_python_date_matches_strptime_edge_cases() {
        // Both zero-padded and unpadded month/day parse, like strptime.
        assert_eq!(parse_python_date("08/02/2025"), Some(d("2025-08-02")));
        assert_eq!(parse_python_date("8/2/2025"), Some(d("2025-08-02")));
        assert_eq!(parse_python_date("2025-8-2"), Some(d("2025-08-02")));
        // Rejections mirroring the CPython TimeRE regexes.
        assert_eq!(parse_python_date("13/1/2025"), None); // month 13
        assert_eq!(parse_python_date("0/5/2025"), None); // month 0
        assert_eq!(parse_python_date("8/2/25"), None); // %Y needs 4 digits
        assert_eq!(parse_python_date("2025-08-15T00:00"), None); // trailing junk
        assert_eq!(parse_python_date(""), None);
    }

    #[test]
    fn python_round2_ties_to_even() {
        assert_eq!(python_round2(3.125), 3.12); // Python: round(3.125, 2) == 3.12
        assert_eq!(python_round2(3.135), 3.13); // Python: round(3.135, 2) == 3.13
        assert_eq!(python_round2(2.675), 2.67); // Python: round(2.675, 2) == 2.67
        assert_eq!(python_round2(25.0 / 8.0), 3.12);
        assert_eq!(python_round2(11.0 / 3.0), 3.67);
        assert_eq!(python_round2(3.5), 3.5);
    }
}
