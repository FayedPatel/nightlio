//! Port of `api/database_stats.py` (`StatisticsMixin`). Owned by the stats
//! data-mixin agent — no other agent edits this file.
//!
//! All aggregation happens in SQL so the queries stay correct and fast as the
//! entry count grows; Rust only does final shaping (and the square root for
//! volatility, since SQLite has no stddev function).
//!
//! Every query filters by `user_id` — per-user isolation is load-bearing now
//! that multi-user auth has shipped.
//!
//! Mood entry dates are stored as text and arrive in two shapes: ISO
//! (`YYYY-MM-DD`) and US locale (`M/D/YYYY`, what the frontend's
//! `toLocaleDateString()` produces). The [`ISO_DAY_EXPR`] fragment normalises
//! both to ISO inside SQL; rows whose date matches neither shape are excluded
//! from aggregates, mirroring how the streak calculator skips unparseable
//! dates. `goal_completions.date` is always written as ISO by the backend, so
//! it needs no normalisation.
//!
//! Every SQL string below is a byte-for-byte copy of the text the Python
//! builds (f-string interpolation is reproduced with `format!`). This is
//! deliberate: even a semantically "equivalent" rewrite of `ISO_DAY_EXPR` or
//! the gaps-and-islands streak query could include/exclude different rows and
//! silently change users' displayed numbers. The
//! `sql_matches_python_source_byte_for_byte` test extracts the strings from
//! `api/database_stats.py` at compile time and pins the equality.
//!
//! Functions take a plain [`Connection`] and are fully blocking — callers run
//! them under `tokio::task::spawn_blocking`.

use std::collections::HashMap;
use std::sync::LazyLock;

use rusqlite::{Connection, named_params};
use serde::Serialize;

use super::common::DatabaseError;

/// Weekday labels indexed by `strftime('%w')` (0 = Sunday). Port of
/// `database_stats.WEEKDAY_NAMES`.
pub const WEEKDAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// The trailing calendar window `mood_volatility` uses when the caller does
/// not override it (Python default argument `days: int = 30`).
pub const DEFAULT_VOLATILITY_WINDOW_DAYS: i64 = 30;

/// Normalise a stored `mood_entries.date` value to ISO `YYYY-MM-DD`, or NULL
/// if it matches neither the ISO nor the US `M/D/YYYY` shape. Verbatim copy
/// of `database_stats._ISO_DAY_EXPR`.
pub const ISO_DAY_EXPR: &str = "
    CASE
        WHEN date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]' THEN date
        WHEN date GLOB '*/*/[0-9][0-9][0-9][0-9]' THEN
            substr(date, -4)
            || '-'
            || printf('%02d', CAST(substr(date, 1, instr(date, '/') - 1) AS INTEGER))
            || '-'
            || printf(
                   '%02d',
                   CAST(
                       substr(
                           substr(date, instr(date, '/') + 1),
                           1,
                           instr(substr(date, instr(date, '/') + 1), '/') - 1
                       ) AS INTEGER
                   )
               )
        ELSE NULL
    END
";

/// Shared CTE: one user's mood entries with a normalised ISO day column.
/// Verbatim copy of `database_stats._ENTRIES_CTE` (an f-string, so it is
/// assembled here with `format!` exactly as Python assembles it).
pub static ENTRIES_CTE: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
    entries AS (
        SELECT id, day, mood
          FROM (
                SELECT id, {ISO_DAY_EXPR} AS day, mood
                  FROM mood_entries
                 WHERE user_id = :user_id
          )
         WHERE day IS NOT NULL
    )
"
    )
});

// --- Query strings (each one the exact f-string body from the Python) -------

static ROLLING_AVERAGES_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
                WITH {cte},
                daily AS (
                    SELECT day,
                           AVG(mood) AS average_mood,
                           COUNT(*) AS entry_count
                      FROM entries
                     GROUP BY day
                )
                SELECT day AS date,
                       average_mood,
                       entry_count,
                       AVG(average_mood) OVER (
                           ORDER BY day
                           ROWS BETWEEN 6 PRECEDING AND CURRENT ROW
                       ) AS rolling_7,
                       AVG(average_mood) OVER (
                           ORDER BY day
                           ROWS BETWEEN 29 PRECEDING AND CURRENT ROW
                       ) AS rolling_30
                  FROM daily
                 ORDER BY day
                ",
        cte = ENTRIES_CTE.as_str()
    )
});

static WEEKDAY_AVERAGES_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
                WITH {cte}
                SELECT CAST(strftime('%w', day) AS INTEGER) AS weekday,
                       AVG(mood) AS average_mood,
                       COUNT(*) AS entry_count
                  FROM entries
                 GROUP BY weekday
                ",
        cte = ENTRIES_CTE.as_str()
    )
});

static MOOD_VOLATILITY_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
                WITH {cte}
                SELECT COUNT(*) AS n,
                       SUM(mood) AS total,
                       SUM(mood * mood) AS total_squares
                  FROM entries
                 WHERE day >= date('now', 'localtime', :window_offset)
                ",
        cte = ENTRIES_CTE.as_str()
    )
});

static TAG_CORRELATIONS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
                WITH {cte},
                option_days AS (
                    SELECT DISTINCT es.option_id, e.day
                      FROM entry_selections es
                      JOIN entries e ON e.id = es.entry_id
                ),
                user_options AS (
                    SELECT DISTINCT go.id AS option_id,
                           go.name AS option_name,
                           g.id AS group_id,
                           g.name AS group_name
                      FROM option_days od
                      JOIN group_options go ON go.id = od.option_id
                      JOIN groups g ON g.id = go.group_id
                     WHERE g.user_id = :user_id
                )
                SELECT uo.option_id,
                       uo.option_name,
                       uo.group_id,
                       uo.group_name,
                       AVG(CASE WHEN od.day IS NOT NULL THEN e.mood END)
                           AS average_mood_selected,
                       COUNT(CASE WHEN od.day IS NOT NULL THEN 1 END)
                           AS entry_count_selected,
                       AVG(CASE WHEN od.day IS NULL THEN e.mood END)
                           AS average_mood_not_selected,
                       COUNT(CASE WHEN od.day IS NULL THEN 1 END)
                           AS entry_count_not_selected
                  FROM user_options uo
                 CROSS JOIN entries e
                  LEFT JOIN option_days od
                         ON od.option_id = uo.option_id AND od.day = e.day
                 GROUP BY uo.option_id, uo.option_name, uo.group_id, uo.group_name
                 ORDER BY uo.group_name, uo.option_name
                ",
        cte = ENTRIES_CTE.as_str()
    )
});

static GOAL_CORRELATIONS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
                WITH {cte},
                user_goals AS (
                    SELECT id AS goal_id, title AS goal_name
                      FROM goals
                     WHERE user_id = :user_id
                ),
                completion_days AS (
                    SELECT DISTINCT goal_id, date AS day
                      FROM goal_completions
                     WHERE user_id = :user_id
                )
                SELECT ug.goal_id,
                       ug.goal_name,
                       AVG(CASE WHEN cd.day IS NOT NULL THEN e.mood END)
                           AS average_mood_completed,
                       COUNT(CASE WHEN cd.day IS NOT NULL THEN 1 END)
                           AS entry_count_completed,
                       AVG(CASE WHEN cd.day IS NULL THEN e.mood END)
                           AS average_mood_not_completed,
                       COUNT(CASE WHEN cd.day IS NULL THEN 1 END)
                           AS entry_count_not_completed
                  FROM user_goals ug
                 CROSS JOIN entries e
                  LEFT JOIN completion_days cd
                         ON cd.goal_id = ug.goal_id AND cd.day = e.day
                 GROUP BY ug.goal_id, ug.goal_name
                 ORDER BY ug.goal_name, ug.goal_id
                ",
        cte = ENTRIES_CTE.as_str()
    )
});

static HEATMAP_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
                WITH {cte}
                SELECT day AS date,
                       AVG(mood) AS average_mood,
                       COUNT(*) AS entry_count
                  FROM entries
                 WHERE day BETWEEN :start AND :end
                 GROUP BY day
                 ORDER BY day
                ",
        cte = ENTRIES_CTE.as_str()
    )
});

static DIGEST_SUMMARY_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
                WITH {cte}
                SELECT COUNT(*) AS entries_logged, AVG(mood) AS average_mood
                  FROM entries
                 WHERE day BETWEEN :start AND :end
                ",
        cte = ENTRIES_CTE.as_str()
    )
});

static DIGEST_PREVIOUS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
                WITH {cte}
                SELECT AVG(mood) AS average_mood
                  FROM entries
                 WHERE day BETWEEN :start AND :end
                ",
        cte = ENTRIES_CTE.as_str()
    )
});

static DIGEST_TOP_TAGS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
                WITH {cte}
                SELECT go.id AS option_id,
                       go.name AS option_name,
                       g.name AS group_name,
                       COUNT(*) AS times_selected
                  FROM entry_selections es
                  JOIN entries e
                        ON e.id = es.entry_id
                       AND e.day BETWEEN :start AND :end
                  JOIN group_options go ON go.id = es.option_id
                  JOIN groups g
                        ON g.id = go.group_id
                       AND g.user_id = :user_id
                 GROUP BY go.id, go.name, g.name
                 ORDER BY times_selected DESC, go.name ASC
                 LIMIT 5
                ",
        cte = ENTRIES_CTE.as_str()
    )
});

static DIGEST_STREAK_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
                WITH {cte},
                logged_days AS (
                    SELECT DISTINCT day
                      FROM entries
                     WHERE day BETWEEN :start AND :end
                ),
                runs AS (
                    SELECT julianday(day)
                           - ROW_NUMBER() OVER (ORDER BY day) AS island
                      FROM logged_days
                )
                SELECT COALESCE(MAX(run_length), 0) AS longest
                  FROM (
                        SELECT COUNT(*) AS run_length
                          FROM runs
                         GROUP BY island
                  )
                ",
        cte = ENTRIES_CTE.as_str()
    )
});

// --- Row shapes (field names match the JSON the Flask API emits) ------------

/// One day in the `rolling_averages` series.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RollingAveragePoint {
    pub date: String,
    pub average_mood: Option<f64>,
    pub entry_count: i64,
    pub rolling_7: Option<f64>,
    pub rolling_30: Option<f64>,
}

/// Return shape of [`rolling_averages`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RollingAverages {
    pub series: Vec<RollingAveragePoint>,
    pub count: usize,
}

/// One weekday row of [`weekday_averages`]; all seven always present.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WeekdayAverage {
    pub weekday: i64,
    pub name: &'static str,
    pub average_mood: Option<f64>,
    pub entry_count: i64,
}

/// Return shape of [`mood_volatility`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MoodVolatility {
    pub window_days: i64,
    pub entry_count: i64,
    pub average_mood: Option<f64>,
    pub stddev: Option<f64>,
}

/// One row of [`tag_correlations`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TagCorrelation {
    pub option_id: i64,
    pub option_name: String,
    pub group_id: i64,
    pub group_name: String,
    pub average_mood_selected: Option<f64>,
    pub entry_count_selected: i64,
    pub average_mood_not_selected: Option<f64>,
    pub entry_count_not_selected: i64,
}

/// One row of [`goal_correlations`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GoalCorrelation {
    pub goal_id: i64,
    pub goal_name: String,
    pub average_mood_completed: Option<f64>,
    pub entry_count_completed: i64,
    pub average_mood_not_completed: Option<f64>,
    pub entry_count_not_completed: i64,
}

/// One logged day of [`heatmap`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HeatmapDay {
    pub date: String,
    pub average_mood: Option<f64>,
    pub entry_count: i64,
}

/// Return shape of [`heatmap`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Heatmap {
    pub year: i64,
    pub days: Vec<HeatmapDay>,
    pub days_logged: usize,
}

/// One of the top-5 tags in [`monthly_digest`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DigestTag {
    pub option_id: i64,
    pub option_name: String,
    pub group_name: String,
    pub times_selected: i64,
}

/// Return shape of [`monthly_digest`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonthlyDigest {
    pub year: i64,
    pub month: i64,
    pub entries_logged: i64,
    pub average_mood: Option<f64>,
    pub previous_average_mood: Option<f64>,
    pub mood_trend: Option<f64>,
    pub top_tags: Vec<DigestTag>,
    pub longest_streak: i64,
}

// --- Query methods -----------------------------------------------------------

/// Daily mood series with 7-day and 30-day trailing means.
///
/// The trailing means are computed in SQL with window functions over the
/// preceding logged days (ROWS BETWEEN), so a day with multiple entries
/// contributes its daily average exactly once.
pub fn rolling_averages(conn: &Connection, user_id: i64) -> Result<RollingAverages, DatabaseError> {
    let mut stmt = conn.prepare(&ROLLING_AVERAGES_SQL)?;
    let series = stmt
        .query_map(named_params! {":user_id": user_id}, |row| {
            Ok(RollingAveragePoint {
                date: row.get("date")?,
                average_mood: row.get("average_mood")?,
                entry_count: row.get("entry_count")?,
                rolling_7: row.get("rolling_7")?,
                rolling_30: row.get("rolling_30")?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let count = series.len();
    Ok(RollingAverages { series, count })
}

/// Average mood and entry count per weekday (0 = Sunday .. 6 = Saturday).
///
/// Always returns all seven weekdays; days with no entries carry a null
/// average and a zero count so callers never divide by zero.
pub fn weekday_averages(
    conn: &Connection,
    user_id: i64,
) -> Result<Vec<WeekdayAverage>, DatabaseError> {
    let mut stmt = conn.prepare(&WEEKDAY_AVERAGES_SQL)?;
    // `weekday` is read as Option: a GLOB-passing but invalid calendar date
    // (e.g. `2025-99-99`) makes strftime return NULL. Python parks such rows
    // under a `None` dict key that the 0..7 loop never reads; skipping them
    // here is the same behavior.
    let mut by_weekday: HashMap<i64, (Option<f64>, i64)> = HashMap::new();
    let mut rows = stmt.query(named_params! {":user_id": user_id})?;
    while let Some(row) = rows.next()? {
        if let Some(weekday) = row.get::<_, Option<i64>>("weekday")? {
            by_weekday.insert(weekday, (row.get("average_mood")?, row.get("entry_count")?));
        }
    }
    Ok((0..7)
        .map(|weekday| {
            let row = by_weekday.get(&weekday);
            WeekdayAverage {
                weekday,
                name: WEEKDAY_NAMES[weekday as usize],
                average_mood: row.and_then(|(avg, _)| *avg),
                entry_count: row.map_or(0, |(_, count)| *count),
            }
        })
        .collect())
}

/// Sample standard deviation of mood over a trailing calendar window.
///
/// The window covers today and the preceding `days - 1` calendar days,
/// evaluated in local time because entry dates are written from the client's
/// local calendar (`toLocaleDateString()`) — using UTC here would shift the
/// window boundary for users in non-UTC timezones. SQLite has no stddev
/// aggregate, so one SQL pass collects n, the sum, and the sum of squares;
/// Rust finishes with the square root (matching the Python, which finishes
/// with `math.sqrt`). The stddev is null when fewer than two entries fall in
/// the window.
///
/// `days < 1` returns the Python `ValueError` message verbatim
/// (`days must be a positive integer`); route callers map it to HTTP 400.
pub fn mood_volatility(
    conn: &Connection,
    user_id: i64,
    days: i64,
) -> Result<MoodVolatility, DatabaseError> {
    if days < 1 {
        return Err(DatabaseError::Message(
            "days must be a positive integer".to_string(),
        ));
    }
    let window_offset = format!("-{} days", days - 1);
    let (n, total, total_squares) = conn.query_row(
        &MOOD_VOLATILITY_SQL,
        named_params! {":user_id": user_id, ":window_offset": window_offset},
        |row| {
            // `f64` reads accept both INTEGER and REAL: SUM(mood) is REAL
            // whenever a schema-legal REAL mood (e.g. 4.5, which passes the
            // 1..=5 CHECK under SQLite's dynamic typing) falls in the
            // window — a strict i64 read 500ed where Flask serves 200.
            Ok((
                row.get::<_, i64>("n")?,
                row.get::<_, Option<f64>>("total")?,
                row.get::<_, Option<f64>>("total_squares")?,
            ))
        },
    )?;

    let average_mood = if n > 0 {
        Some(total.unwrap_or(0.0) / n as f64)
    } else {
        None
    };
    let stddev = if n >= 2 {
        let total = total.unwrap_or(0.0);
        let total_squares = total_squares.unwrap_or(0.0);
        let variance = (total_squares - total * total / n as f64) / (n as f64 - 1.0);
        // Floating point can push a zero variance a hair negative.
        Some(variance.max(0.0).sqrt())
    } else {
        None
    };
    Ok(MoodVolatility {
        window_days: days,
        entry_count: n,
        average_mood,
        stddev,
    })
}

/// Per group option: avg mood and entry count on days it was selected versus
/// days it was not.
///
/// A day counts as "selected" for an option when any of the user's entries on
/// that day has the option selected; every entry on such a day lands on the
/// "selected" side. Only options the user has actually selected at least once
/// appear, and the owning group is explicitly filtered by `user_id` so a
/// selection pointing at another user's option can never leak that user's
/// group names or ids. Counts accompany every average so the UI can suppress
/// noise from tiny samples; no p-values, by design.
pub fn tag_correlations(
    conn: &Connection,
    user_id: i64,
) -> Result<Vec<TagCorrelation>, DatabaseError> {
    let mut stmt = conn.prepare(&TAG_CORRELATIONS_SQL)?;
    let rows = stmt
        .query_map(named_params! {":user_id": user_id}, |row| {
            Ok(TagCorrelation {
                option_id: row.get("option_id")?,
                option_name: row.get("option_name")?,
                group_id: row.get("group_id")?,
                group_name: row.get("group_name")?,
                average_mood_selected: row.get("average_mood_selected")?,
                entry_count_selected: row.get("entry_count_selected")?,
                average_mood_not_selected: row.get("average_mood_not_selected")?,
                entry_count_not_selected: row.get("entry_count_not_selected")?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Per goal: avg mood and entry count on days the goal was completed versus
/// days it was not. Same shape as tag correlations.
///
/// Goals with no completions still appear (null completed average, zero
/// count); a user with no mood entries gets an empty list.
pub fn goal_correlations(
    conn: &Connection,
    user_id: i64,
) -> Result<Vec<GoalCorrelation>, DatabaseError> {
    let mut stmt = conn.prepare(&GOAL_CORRELATIONS_SQL)?;
    let rows = stmt
        .query_map(named_params! {":user_id": user_id}, |row| {
            Ok(GoalCorrelation {
                goal_id: row.get("goal_id")?,
                goal_name: row.get("goal_name")?,
                average_mood_completed: row.get("average_mood_completed")?,
                entry_count_completed: row.get("entry_count_completed")?,
                average_mood_not_completed: row.get("average_mood_not_completed")?,
                entry_count_not_completed: row.get("entry_count_not_completed")?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Average mood and entry count for every logged day of a year.
pub fn heatmap(conn: &Connection, user_id: i64, year: i64) -> Result<Heatmap, DatabaseError> {
    let start = format!("{year:04}-01-01");
    let end = format!("{year:04}-12-31");
    let mut stmt = conn.prepare(&HEATMAP_SQL)?;
    let days = stmt
        .query_map(
            named_params! {":user_id": user_id, ":start": start, ":end": end},
            |row| {
                Ok(HeatmapDay {
                    date: row.get("date")?,
                    average_mood: row.get("average_mood")?,
                    entry_count: row.get("entry_count")?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let days_logged = days.len();
    Ok(Heatmap {
        year,
        days,
        days_logged,
    })
}

/// True for Gregorian leap years — the same predicate `calendar.isleap` uses.
fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// `calendar.monthrange(year, month)[1]` for month in 1..=12.
fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 31,
    }
}

/// Month in review: entries logged, average mood, trend versus the previous
/// month, top 5 tags by frequency, and the longest streak of consecutive
/// logged days within the month.
///
/// `mood_trend` is the signed difference between this month's average and the
/// previous month's, or null when either month has no entries. Built entirely
/// from existing data — no new privacy surface.
///
/// An out-of-range month returns the Python `ValueError` message verbatim
/// (`month must be between 1 and 12`); route callers map it to HTTP 400.
pub fn monthly_digest(
    conn: &Connection,
    user_id: i64,
    year: i64,
    month: i64,
) -> Result<MonthlyDigest, DatabaseError> {
    if !(1..=12).contains(&month) {
        return Err(DatabaseError::Message(
            "month must be between 1 and 12".to_string(),
        ));
    }

    let start = format!("{year:04}-{month:02}-01");
    let end = format!("{year:04}-{month:02}-{:02}", days_in_month(year, month));
    let (prev_year, prev_month) = if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    };
    let prev_start = format!("{prev_year:04}-{prev_month:02}-01");
    let prev_end = format!(
        "{prev_year:04}-{prev_month:02}-{:02}",
        days_in_month(prev_year, prev_month)
    );

    let (entries_logged, average_mood) = conn.query_row(
        &DIGEST_SUMMARY_SQL,
        named_params! {":user_id": user_id, ":start": start, ":end": end},
        |row| {
            Ok((
                row.get::<_, i64>("entries_logged")?,
                row.get::<_, Option<f64>>("average_mood")?,
            ))
        },
    )?;

    let previous_average_mood: Option<f64> = conn.query_row(
        &DIGEST_PREVIOUS_SQL,
        named_params! {":user_id": user_id, ":start": prev_start, ":end": prev_end},
        |row| row.get("average_mood"),
    )?;

    let mut stmt = conn.prepare(&DIGEST_TOP_TAGS_SQL)?;
    let top_tags = stmt
        .query_map(
            named_params! {":user_id": user_id, ":start": start, ":end": end},
            |row| {
                Ok(DigestTag {
                    option_id: row.get("option_id")?,
                    option_name: row.get("option_name")?,
                    group_name: row.get("group_name")?,
                    times_selected: row.get("times_selected")?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // Gaps-and-islands: consecutive days share the same
    // julianday - row_number value, so the biggest island is the longest
    // streak within the month.
    let longest_streak: i64 = conn.query_row(
        &DIGEST_STREAK_SQL,
        named_params! {":user_id": user_id, ":start": start, ":end": end},
        |row| row.get("longest"),
    )?;

    let mood_trend = match (average_mood, previous_average_mood) {
        (Some(current), Some(previous)) => Some(current - previous),
        _ => None,
    };

    Ok(MonthlyDigest {
        year,
        month,
        entries_logged,
        average_mood,
        previous_average_mood,
        mood_trend,
        top_tags,
        longest_streak,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{SelfHostSeed, bootstrap, connect};
    use chrono::Datelike;
    use serde_json::Value;

    /// The Python source this module ports. The live `api/` tree is gone
    /// (legacy removal, contract/DECISIONS.md) — this frozen copy is the
    /// golden the SQL-parity test pins against, same status as the contract
    /// fixtures: never regenerated, edited only with an owner-approved
    /// contract change.
    const PYTHON_SOURCE: &str = include_str!("../../tests/fixtures/database_stats.py.frozen");

    /// Seed rows shared verbatim with the Python fixture generator that
    /// produced the pinned JSON below (user 1 is the bootstrap-seeded
    /// self-host user; users 2 and 3 are inserted here). Covers: mixed
    /// ISO / US date shapes, an unparseable date (`Aug 5, 2025`) that every
    /// aggregate must silently exclude, two entries on one day, a selection
    /// pointing at another user's option (leak check), and a goal with no
    /// completions.
    const SEED_SQL: &str = "
INSERT INTO users (id, google_id, email, name) VALUES (2, 'user-two', 'two@example.com', 'Two');
INSERT INTO users (id, google_id, email, name) VALUES (3, 'user-three', 'three@example.com', 'Three');

INSERT INTO mood_entries (id, user_id, date, mood, content) VALUES (101, 1, '2025-08-01', 4, 'e101');
INSERT INTO mood_entries (id, user_id, date, mood, content) VALUES (102, 1, '8/2/2025', 2, 'e102');
INSERT INTO mood_entries (id, user_id, date, mood, content) VALUES (103, 1, '2025-08-03', 5, 'e103');
INSERT INTO mood_entries (id, user_id, date, mood, content) VALUES (104, 1, '2025-08-03', 3, 'e104');
INSERT INTO mood_entries (id, user_id, date, mood, content) VALUES (105, 1, 'Aug 5, 2025', 1, 'e105');
INSERT INTO mood_entries (id, user_id, date, mood, content) VALUES (106, 1, '2025-07-30', 3, 'e106');
INSERT INTO mood_entries (id, user_id, date, mood, content) VALUES (107, 1, '12/31/2024', 5, 'e107');
INSERT INTO mood_entries (id, user_id, date, mood, content) VALUES (201, 2, '2025-08-01', 1, 'u2');

INSERT INTO groups (id, user_id, name) VALUES (100, 1, 'Fitness');
INSERT INTO groups (id, user_id, name) VALUES (101, 1, 'Social');
INSERT INTO groups (id, user_id, name) VALUES (200, 2, 'Other');
INSERT INTO group_options (id, group_id, name) VALUES (100, 100, 'Exercise');
INSERT INTO group_options (id, group_id, name) VALUES (101, 101, 'Friends');
INSERT INTO group_options (id, group_id, name) VALUES (200, 200, 'Foreign');

INSERT INTO entry_selections (entry_id, option_id) VALUES (101, 100);
INSERT INTO entry_selections (entry_id, option_id) VALUES (103, 100);
INSERT INTO entry_selections (entry_id, option_id) VALUES (103, 101);
INSERT INTO entry_selections (entry_id, option_id) VALUES (102, 200);
INSERT INTO entry_selections (entry_id, option_id) VALUES (105, 101);

INSERT INTO goals (id, user_id, title, frequency_per_week) VALUES (100, 1, 'Meditate', 3);
INSERT INTO goals (id, user_id, title, frequency_per_week) VALUES (101, 1, 'Run', 2);
INSERT INTO goals (id, user_id, title, frequency_per_week) VALUES (200, 2, 'Sleep', 1);
INSERT INTO goal_completions (user_id, goal_id, date) VALUES (1, 100, '2025-08-01');
INSERT INTO goal_completions (user_id, goal_id, date) VALUES (1, 100, '2025-08-03');
INSERT INTO goal_completions (user_id, goal_id, date) VALUES (2, 200, '2025-08-02');
";

    /// User 3 gets entries dated relative to today so the volatility window
    /// (trailing 30 days from server-local now) is deterministic whenever the
    /// suite runs. Offsets stay well inside/outside the window so a midnight
    /// rollover between seeding and querying cannot flip a row.
    /// Mirrors `seed_volatility_user` in the Python fixture generator.
    fn seed_volatility_entries(conn: &Connection) {
        let today = chrono::Local::now().date_naive();
        let rows: [(i64, u64, i64, bool); 5] = [
            (301, 0, 5, true),
            (302, 1, 3, true),
            (303, 2, 4, false), // US M/D/YYYY shape, still inside the window
            (304, 5, 1, true),
            (305, 40, 5, true), // outside the 30-day window
        ];
        for (id, offset, mood, iso) in rows {
            let day = today - chrono::Days::new(offset);
            let text = if iso {
                day.format("%Y-%m-%d").to_string()
            } else {
                format!("{}/{}/{}", day.month(), day.day(), day.year())
            };
            conn.execute(
                "INSERT INTO mood_entries (id, user_id, date, mood, content) \
                 VALUES (?1, 3, ?2, ?3, ?4)",
                rusqlite::params![id, text, mood, format!("v{id}")],
            )
            .unwrap();
        }
    }

    fn seeded_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.db");
        let path = path.to_str().unwrap();
        bootstrap(path, &SelfHostSeed::default()).unwrap();
        let conn = connect(path).unwrap();

        // The pinned fixtures assume the bootstrap-seeded self-host user got
        // id 1, exactly as the Python `_ensure_default_user` produces.
        let default_id: i64 = conn
            .query_row(
                "SELECT id FROM users WHERE google_id = 'selfhost_default_user'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(default_id, 1);

        conn.execute_batch(SEED_SQL).unwrap();
        seed_volatility_entries(&conn);
        (dir, conn)
    }

    /// Assert a serialized result equals JSON pinned from the Python side.
    ///
    /// Every pinned literal below was produced by running the real
    /// `api/database_stats.py` (via `MoodDatabase`, under
    /// `api/venv/bin/python`, SQLite 3.37.2) against a database seeded with
    /// exactly the rows above. Floats are Python `repr` output, so they
    /// round-trip to the identical f64 the comparison sees.
    fn assert_matches_python<T: Serialize>(actual: &T, pinned: &str) {
        let actual = serde_json::to_value(actual).unwrap();
        let expected: Value = serde_json::from_str(pinned).unwrap();
        assert_eq!(actual, expected);
    }

    // --- Behavioral parity, pinned against Python-computed values -----------

    #[test]
    fn rolling_averages_matches_python() {
        let (_dir, conn) = seeded_db();
        let result = rolling_averages(&conn, 1).unwrap();
        // The US-format 12/31/2024 and 8/2/2025 entries are normalized into
        // the series; the unparseable 'Aug 5, 2025' entry is excluded; the
        // two 2025-08-03 entries collapse into one day with average 4.0.
        assert_matches_python(
            &result,
            r#"{
              "count": 5,
              "series": [
                {"average_mood": 5.0, "date": "2024-12-31", "entry_count": 1, "rolling_30": 5.0, "rolling_7": 5.0},
                {"average_mood": 3.0, "date": "2025-07-30", "entry_count": 1, "rolling_30": 4.0, "rolling_7": 4.0},
                {"average_mood": 4.0, "date": "2025-08-01", "entry_count": 1, "rolling_30": 4.0, "rolling_7": 4.0},
                {"average_mood": 2.0, "date": "2025-08-02", "entry_count": 1, "rolling_30": 3.5, "rolling_7": 3.5},
                {"average_mood": 4.0, "date": "2025-08-03", "entry_count": 2, "rolling_30": 3.6, "rolling_7": 3.6}
              ]
            }"#,
        );
    }

    #[test]
    fn weekday_averages_matches_python() {
        let (_dir, conn) = seeded_db();
        let result = weekday_averages(&conn, 1).unwrap();
        assert_matches_python(
            &result,
            r#"[
              {"average_mood": 4.0,  "entry_count": 2, "name": "Sunday",    "weekday": 0},
              {"average_mood": null, "entry_count": 0, "name": "Monday",    "weekday": 1},
              {"average_mood": 5.0,  "entry_count": 1, "name": "Tuesday",   "weekday": 2},
              {"average_mood": 3.0,  "entry_count": 1, "name": "Wednesday", "weekday": 3},
              {"average_mood": null, "entry_count": 0, "name": "Thursday",  "weekday": 4},
              {"average_mood": 4.0,  "entry_count": 1, "name": "Friday",    "weekday": 5},
              {"average_mood": 2.0,  "entry_count": 1, "name": "Saturday",  "weekday": 6}
            ]"#,
        );
    }

    #[test]
    fn mood_volatility_matches_python() {
        let (_dir, conn) = seeded_db();

        // User 3: four entries inside the trailing 30-day window (moods
        // 5, 3, 4, 1 — one in US date format), one outside it.
        let result = mood_volatility(&conn, 3, DEFAULT_VOLATILITY_WINDOW_DAYS).unwrap();
        assert_matches_python(
            &result,
            r#"{"average_mood": 3.25, "entry_count": 4, "stddev": 1.707825127659933, "window_days": 30}"#,
        );

        // User 1's entries are all far in the past: empty window, null stats.
        let result = mood_volatility(&conn, 1, DEFAULT_VOLATILITY_WINDOW_DAYS).unwrap();
        assert_matches_python(
            &result,
            r#"{"average_mood": null, "entry_count": 0, "stddev": null, "window_days": 30}"#,
        );
    }

    #[test]
    fn mood_volatility_tolerates_schema_legal_real_mood() {
        let (_dir, conn) = seeded_db();
        // SQLite dynamic typing: 4.5 passes the 1..=5 CHECK and stays REAL
        // under INTEGER affinity, so SUM(mood)/SUM(mood*mood) come back REAL.
        // Previously the i64 reads 500ed with InvalidColumnType where Flask
        // serves 200. Dates are relative so the rows sit inside the window.
        conn.execute(
            "INSERT INTO users (id, google_id, email, name) \
             VALUES (4, 'user-four', 'four@example.com', 'Four')",
            [],
        )
        .unwrap();
        let today = chrono::Local::now().date_naive();
        for (offset, mood_sql) in [(0u64, "2"), (1, "4.5"), (2, "4")] {
            let day = (today - chrono::Days::new(offset)).format("%Y-%m-%d");
            conn.execute(
                &format!(
                    "INSERT INTO mood_entries (user_id, date, mood, content) \
                     VALUES (4, '{day}', {mood_sql}, 'r')"
                ),
                [],
            )
            .unwrap();
        }
        // Pinned from Python (scratchpad real_mood_xcheck.py, 2026-08-15):
        // n=3, total=10.5, total_squares=40.25 -> average 3.5,
        // stddev sqrt(1.75) = 1.3228756555322954.
        let result = mood_volatility(&conn, 4, DEFAULT_VOLATILITY_WINDOW_DAYS).unwrap();
        assert_matches_python(
            &result,
            r#"{"average_mood": 3.5, "entry_count": 3, "stddev": 1.3228756555322954, "window_days": 30}"#,
        );
    }

    #[test]
    fn mood_volatility_rejects_non_positive_days() {
        let (_dir, conn) = seeded_db();
        for days in [0, -3] {
            let err = mood_volatility(&conn, 1, days).unwrap_err();
            // Verbatim Python ValueError text; the route layer maps it to 400.
            assert_eq!(err.to_string(), "days must be a positive integer");
        }
    }

    #[test]
    fn tag_correlations_matches_python() {
        let (_dir, conn) = seeded_db();
        let result = tag_correlations(&conn, 1).unwrap();
        // Option 200 belongs to user 2's group: selected on one of user 1's
        // entries, but the g.user_id filter keeps it out (no leak). The
        // selection on the unparseable-date entry 105 contributes nothing.
        assert_matches_python(
            &result,
            r#"[
              {
                "average_mood_not_selected": 3.3333333333333335,
                "average_mood_selected": 4.0,
                "entry_count_not_selected": 3,
                "entry_count_selected": 3,
                "group_id": 100,
                "group_name": "Fitness",
                "option_id": 100,
                "option_name": "Exercise"
              },
              {
                "average_mood_not_selected": 3.5,
                "average_mood_selected": 4.0,
                "entry_count_not_selected": 4,
                "entry_count_selected": 2,
                "group_id": 101,
                "group_name": "Social",
                "option_id": 101,
                "option_name": "Friends"
              }
            ]"#,
        );

        // User 2 has entries but no selections of their own options.
        let result = tag_correlations(&conn, 2).unwrap();
        assert_matches_python(&result, "[]");
    }

    #[test]
    fn goal_correlations_matches_python() {
        let (_dir, conn) = seeded_db();
        let result = goal_correlations(&conn, 1).unwrap();
        // 'Run' has no completions: null completed average, zero count.
        assert_matches_python(
            &result,
            r#"[
              {
                "average_mood_completed": 4.0,
                "average_mood_not_completed": 3.3333333333333335,
                "entry_count_completed": 3,
                "entry_count_not_completed": 3,
                "goal_id": 100,
                "goal_name": "Meditate"
              },
              {
                "average_mood_completed": null,
                "average_mood_not_completed": 3.6666666666666665,
                "entry_count_completed": 0,
                "entry_count_not_completed": 6,
                "goal_id": 101,
                "goal_name": "Run"
              }
            ]"#,
        );

        // User 2 completed their goal on a day with no mood entry.
        let result = goal_correlations(&conn, 2).unwrap();
        assert_matches_python(
            &result,
            r#"[
              {
                "average_mood_completed": null,
                "average_mood_not_completed": 1.0,
                "entry_count_completed": 0,
                "entry_count_not_completed": 1,
                "goal_id": 200,
                "goal_name": "Sleep"
              }
            ]"#,
        );
    }

    #[test]
    fn heatmap_matches_python() {
        let (_dir, conn) = seeded_db();
        let result = heatmap(&conn, 1, 2025).unwrap();
        assert_matches_python(
            &result,
            r#"{
              "days": [
                {"average_mood": 3.0, "date": "2025-07-30", "entry_count": 1},
                {"average_mood": 4.0, "date": "2025-08-01", "entry_count": 1},
                {"average_mood": 2.0, "date": "2025-08-02", "entry_count": 1},
                {"average_mood": 4.0, "date": "2025-08-03", "entry_count": 2}
              ],
              "days_logged": 4,
              "year": 2025
            }"#,
        );

        // The US-format 12/31/2024 entry lands in the 2024 heatmap.
        let result = heatmap(&conn, 1, 2024).unwrap();
        assert_matches_python(
            &result,
            r#"{
              "days": [{"average_mood": 5.0, "date": "2024-12-31", "entry_count": 1}],
              "days_logged": 1,
              "year": 2024
            }"#,
        );

        // Per-user isolation.
        let result = heatmap(&conn, 2, 2025).unwrap();
        assert_matches_python(
            &result,
            r#"{
              "days": [{"average_mood": 1.0, "date": "2025-08-01", "entry_count": 1}],
              "days_logged": 1,
              "year": 2025
            }"#,
        );
    }

    #[test]
    fn monthly_digest_matches_python() {
        let (_dir, conn) = seeded_db();

        // August 2025: 4 entries over a 3-day streak, previous-month trend,
        // top tags ordered by times_selected DESC then name ASC.
        let result = monthly_digest(&conn, 1, 2025, 8).unwrap();
        assert_matches_python(
            &result,
            r#"{
              "average_mood": 3.5,
              "entries_logged": 4,
              "longest_streak": 3,
              "month": 8,
              "mood_trend": 0.5,
              "previous_average_mood": 3.0,
              "top_tags": [
                {"group_name": "Fitness", "option_id": 100, "option_name": "Exercise", "times_selected": 2},
                {"group_name": "Social", "option_id": 101, "option_name": "Friends", "times_selected": 1}
              ],
              "year": 2025
            }"#,
        );

        // July 2025: one entry, June empty so no trend.
        let result = monthly_digest(&conn, 1, 2025, 7).unwrap();
        assert_matches_python(
            &result,
            r#"{
              "average_mood": 3.0,
              "entries_logged": 1,
              "longest_streak": 1,
              "month": 7,
              "mood_trend": null,
              "previous_average_mood": null,
              "top_tags": [],
              "year": 2025
            }"#,
        );

        // December 2024: the US-format entry counts; November empty.
        let result = monthly_digest(&conn, 1, 2024, 12).unwrap();
        assert_matches_python(
            &result,
            r#"{
              "average_mood": 5.0,
              "entries_logged": 1,
              "longest_streak": 1,
              "month": 12,
              "mood_trend": null,
              "previous_average_mood": null,
              "top_tags": [],
              "year": 2024
            }"#,
        );

        // June 2025: completely empty month.
        let result = monthly_digest(&conn, 1, 2025, 6).unwrap();
        assert_matches_python(
            &result,
            r#"{
              "average_mood": null,
              "entries_logged": 0,
              "longest_streak": 0,
              "month": 6,
              "mood_trend": null,
              "previous_average_mood": null,
              "top_tags": [],
              "year": 2025
            }"#,
        );

        // January: the previous month rolls over into the prior year, and a
        // null current average keeps the trend null even though the previous
        // month has one.
        let result = monthly_digest(&conn, 1, 2025, 1).unwrap();
        assert_matches_python(
            &result,
            r#"{
              "average_mood": null,
              "entries_logged": 0,
              "longest_streak": 0,
              "month": 1,
              "mood_trend": null,
              "previous_average_mood": 5.0,
              "top_tags": [],
              "year": 2025
            }"#,
        );
    }

    #[test]
    fn monthly_digest_rejects_bad_month() {
        let (_dir, conn) = seeded_db();
        for month in [0, 13, -1] {
            let err = monthly_digest(&conn, 1, 2025, month).unwrap_err();
            // Verbatim Python ValueError text; the route layer maps it to 400.
            assert_eq!(err.to_string(), "month must be between 1 and 12");
        }
    }

    // --- SQL text parity with the Python source ------------------------------

    /// Content of the triple-quoted literal that follows `marker`.
    fn extract_triple_quoted(marker: &str) -> String {
        let start = PYTHON_SOURCE
            .find(marker)
            .unwrap_or_else(|| panic!("marker {marker:?} not found in api/database_stats.py"))
            + marker.len();
        let rest = &PYTHON_SOURCE[start..];
        let end = rest.find("\"\"\"").expect("unterminated triple quote");
        rest[..end].to_string()
    }

    /// Bodies of every `f"""..."""` literal in the Python source, in order.
    fn extract_fstring_bodies() -> Vec<&'static str> {
        let mut bodies = Vec::new();
        let mut rest = PYTHON_SOURCE;
        while let Some(idx) = rest.find("f\"\"\"") {
            let after = &rest[idx + 4..];
            let end = after.find("\"\"\"").expect("unterminated f-string");
            bodies.push(&after[..end]);
            rest = &after[end + 3..];
        }
        bodies
    }

    #[test]
    fn sql_matches_python_source_byte_for_byte() {
        // The normalizer expression and the shared CTE.
        let iso = extract_triple_quoted("_ISO_DAY_EXPR = \"\"\"");
        assert_eq!(ISO_DAY_EXPR, iso, "_ISO_DAY_EXPR drifted");

        let cte_template = extract_triple_quoted("_ENTRIES_CTE = f\"\"\"");
        let cte = cte_template.replace("{_ISO_DAY_EXPR}", &iso);
        assert_eq!(ENTRIES_CTE.as_str(), cte, "_ENTRIES_CTE drifted");

        // Every query f-string in the mixin starts with the same WITH clause;
        // reproduce the interpolation and demand byte equality, in source
        // order: rolling, weekday, volatility, tag corr, goal corr, heatmap,
        // digest summary / previous / top tags / streak.
        let queries: Vec<String> = extract_fstring_bodies()
            .into_iter()
            .filter(|body| body.starts_with("\n                WITH {_ENTRIES_CTE}"))
            .map(|body| body.replace("{_ENTRIES_CTE}", &cte))
            .collect();
        let built: [(&str, &LazyLock<String>); 10] = [
            ("rolling_averages", &ROLLING_AVERAGES_SQL),
            ("weekday_averages", &WEEKDAY_AVERAGES_SQL),
            ("mood_volatility", &MOOD_VOLATILITY_SQL),
            ("tag_correlations", &TAG_CORRELATIONS_SQL),
            ("goal_correlations", &GOAL_CORRELATIONS_SQL),
            ("heatmap", &HEATMAP_SQL),
            ("digest summary", &DIGEST_SUMMARY_SQL),
            ("digest previous", &DIGEST_PREVIOUS_SQL),
            ("digest top tags", &DIGEST_TOP_TAGS_SQL),
            ("digest streak", &DIGEST_STREAK_SQL),
        ];
        assert_eq!(
            queries.len(),
            built.len(),
            "unexpected number of query f-strings in api/database_stats.py"
        );
        for ((name, sql), python) in built.iter().zip(&queries) {
            assert_eq!(
                sql.as_str(),
                python.as_str(),
                "{name} SQL drifted from api/database_stats.py"
            );
        }
    }
}
