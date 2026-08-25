//! PostgreSQL twin of [`crate::db::stats`] (WS4; docs/plans/v0.6.0.md
//! Feature 4, "PG wire-quirk policy").
//!
//! The sync module is graded by displayed numbers (its module doc warns
//! that even a semantically "equivalent" rewrite can change users'
//! numbers), so every translation choice below reproduces an observable
//! SQLite behavior:
//!
//! - [`super::moods::PG_ISO_DAY_EXPR`] rebuilds `_ISO_DAY_EXPR` including
//!   its garbage-date quirks (`13/45/2025` → `2025-13-45` stays a series
//!   point in `rolling_averages`).
//! - `strftime('%w', day)` → `EXTRACT(DOW FROM nightlio_safe_date(day))`
//!   (both 0 = Sunday). `nightlio_safe_date` (0001 baseline,
//!   `pg_input_is_valid`, PG ≥ 16) reproduces strftime-on-garbage NULL:
//!   an ISO-*shaped* but invalid day (`2025-99-99`, `2025-13-45`) yields
//!   a NULL weekday, which the Rust shaping skips exactly like the sync
//!   side.
//! - `julianday(day)` in the gaps-and-islands streak becomes
//!   `(nightlio_safe_date(day) - DATE '0001-01-01')` — consecutive days
//!   still differ by exactly 1, so island grouping is unchanged (the
//!   absolute offset cancels out of `day_number - ROW_NUMBER()`).
//! - `date('now', 'localtime', :window_offset)` is **computed in Rust**
//!   ([`util::local_date_minus_days_text`]) and bound as a text cutoff —
//!   SQLite date modifiers are never translated into Postgres date
//!   arithmetic.
//! - All `BETWEEN`/`>=` filters on `day` stay lexicographic text
//!   comparisons (collation "C" flows through from the `date` column),
//!   matching DECISIONS.md #4.
//! - `SUM(mood)` / `AVG(mood)` read as f64 (the column is double
//!   precision); Rust finishes the volatility math exactly like the sync
//!   port (which mirrors Python's `math.sqrt` finish).
//!
//! Functions follow the `pg/util.rs` conventions: `&impl GenericClient`,
//! errors via [`util::db_error`].

use std::collections::HashMap;
use std::sync::LazyLock;

use tokio_postgres::GenericClient;

use super::moods::PG_ISO_DAY_EXPR;
use super::util;
use crate::db::common::DatabaseError;
use crate::db::stats::{
    DigestTag, GoalCorrelation, Heatmap, HeatmapDay, MonthlyDigest, MoodVolatility,
    RollingAveragePoint, RollingAverages, TagCorrelation, WEEKDAY_NAMES, WeekdayAverage,
};

// --- SQL ---------------------------------------------------------------------

/// Twin of `stats::ENTRIES_CTE`: one user's mood entries with a
/// normalised ISO day column. `$1` is always the user id. (Postgres
/// requires the subquery alias SQLite lets us omit.)
static PG_ENTRIES_CTE: LazyLock<String> = LazyLock::new(|| {
    format!(
        "entries AS (
        SELECT id, day, mood
          FROM (
                SELECT id, {expr} AS day, mood
                  FROM mood_entries
                 WHERE user_id = $1
          ) normalised
         WHERE day IS NOT NULL
    )",
        expr = PG_ISO_DAY_EXPR.as_str()
    )
});

static ROLLING_AVERAGES_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH {cte},
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
                 ORDER BY day",
        cte = PG_ENTRIES_CTE.as_str()
    )
});

static WEEKDAY_AVERAGES_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH {cte}
                SELECT CAST(EXTRACT(DOW FROM nightlio_safe_date(day)) AS bigint) AS weekday,
                       AVG(mood) AS average_mood,
                       COUNT(*) AS entry_count
                  FROM entries
                 GROUP BY weekday",
        cte = PG_ENTRIES_CTE.as_str()
    )
});

/// `$2` is the window cutoff date, computed in Rust
/// (`util::local_date_minus_days_text`, the twin of
/// `date('now', 'localtime', :window_offset)`); the comparison stays
/// lexicographic text like SQLite's.
static MOOD_VOLATILITY_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH {cte}
                SELECT COUNT(*) AS n,
                       SUM(mood) AS total,
                       SUM(mood * mood) AS total_squares
                  FROM entries
                 WHERE day >= $2",
        cte = PG_ENTRIES_CTE.as_str()
    )
});

static TAG_CORRELATIONS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH {cte},
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
                     WHERE g.user_id = $1
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
                 ORDER BY uo.group_name, uo.option_name",
        cte = PG_ENTRIES_CTE.as_str()
    )
});

static GOAL_CORRELATIONS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH {cte},
                user_goals AS (
                    SELECT id AS goal_id, title AS goal_name
                      FROM goals
                     WHERE user_id = $1
                ),
                completion_days AS (
                    SELECT DISTINCT goal_id, date AS day
                      FROM goal_completions
                     WHERE user_id = $1
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
                 ORDER BY ug.goal_name, ug.goal_id",
        cte = PG_ENTRIES_CTE.as_str()
    )
});

static HEATMAP_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH {cte}
                SELECT day AS date,
                       AVG(mood) AS average_mood,
                       COUNT(*) AS entry_count
                  FROM entries
                 WHERE day BETWEEN $2 AND $3
                 GROUP BY day
                 ORDER BY day",
        cte = PG_ENTRIES_CTE.as_str()
    )
});

static DIGEST_SUMMARY_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH {cte}
                SELECT COUNT(*) AS entries_logged, AVG(mood) AS average_mood
                  FROM entries
                 WHERE day BETWEEN $2 AND $3",
        cte = PG_ENTRIES_CTE.as_str()
    )
});

static DIGEST_PREVIOUS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH {cte}
                SELECT AVG(mood) AS average_mood
                  FROM entries
                 WHERE day BETWEEN $2 AND $3",
        cte = PG_ENTRIES_CTE.as_str()
    )
});

static DIGEST_TOP_TAGS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH {cte}
                SELECT go.id AS option_id,
                       go.name AS option_name,
                       g.name AS group_name,
                       COUNT(*) AS times_selected
                  FROM entry_selections es
                  JOIN entries e
                        ON e.id = es.entry_id
                       AND e.day BETWEEN $2 AND $3
                  JOIN group_options go ON go.id = es.option_id
                  JOIN groups g
                        ON g.id = go.group_id
                       AND g.user_id = $1
                 GROUP BY go.id, go.name, g.name
                 ORDER BY times_selected DESC, go.name ASC
                 LIMIT 5",
        cte = PG_ENTRIES_CTE.as_str()
    )
});

/// Gaps-and-islands: `julianday(day)` becomes a day-number via date
/// subtraction; consecutive days share the same `day_number - row_number`
/// island exactly as with julian day floats. (Every `day` inside a month
/// window is a real calendar date — the window's lexicographic BETWEEN
/// cannot admit an invalid one — so `nightlio_safe_date` never returns
/// NULL here.)
static DIGEST_STREAK_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH {cte},
                logged_days AS (
                    SELECT DISTINCT day
                      FROM entries
                     WHERE day BETWEEN $2 AND $3
                ),
                runs AS (
                    SELECT (nightlio_safe_date(day) - DATE '0001-01-01')
                           - ROW_NUMBER() OVER (ORDER BY day) AS island
                      FROM logged_days
                )
                SELECT COALESCE(MAX(run_length), 0) AS longest
                  FROM (
                        SELECT COUNT(*) AS run_length
                          FROM runs
                         GROUP BY island
                  ) islands",
        cte = PG_ENTRIES_CTE.as_str()
    )
});

// --- Query methods -----------------------------------------------------------

/// Twin of `stats::rolling_averages`.
pub async fn rolling_averages(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<RollingAverages, DatabaseError> {
    let rows = client
        .query(ROLLING_AVERAGES_SQL.as_str(), &[&user_id])
        .await
        .map_err(util::db_error)?;
    let series = rows
        .iter()
        .map(|row| {
            Ok(RollingAveragePoint {
                date: row.try_get("date").map_err(util::db_error)?,
                average_mood: row.try_get("average_mood").map_err(util::db_error)?,
                entry_count: row.try_get("entry_count").map_err(util::db_error)?,
                rolling_7: row.try_get("rolling_7").map_err(util::db_error)?,
                rolling_30: row.try_get("rolling_30").map_err(util::db_error)?,
            })
        })
        .collect::<Result<Vec<_>, DatabaseError>>()?;
    let count = series.len();
    Ok(RollingAverages { series, count })
}

/// Twin of `stats::weekday_averages`: all seven weekdays always present;
/// NULL weekdays (garbage dates) are skipped exactly like the sync side.
pub async fn weekday_averages(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<Vec<WeekdayAverage>, DatabaseError> {
    let rows = client
        .query(WEEKDAY_AVERAGES_SQL.as_str(), &[&user_id])
        .await
        .map_err(util::db_error)?;
    let mut by_weekday: HashMap<i64, (Option<f64>, i64)> = HashMap::new();
    for row in &rows {
        if let Some(weekday) = row
            .try_get::<_, Option<i64>>("weekday")
            .map_err(util::db_error)?
        {
            by_weekday.insert(
                weekday,
                (
                    row.try_get("average_mood").map_err(util::db_error)?,
                    row.try_get("entry_count").map_err(util::db_error)?,
                ),
            );
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

/// Twin of `stats::mood_volatility`. The trailing window cutoff is
/// computed in Rust — the exact value SQLite's
/// `date('now', 'localtime', '-N days')` produces — and bound as text.
pub async fn mood_volatility(
    client: &impl GenericClient,
    user_id: i64,
    days: i64,
) -> Result<MoodVolatility, DatabaseError> {
    if days < 1 {
        return Err(DatabaseError::Message(
            "days must be a positive integer".to_string(),
        ));
    }
    let cutoff = util::local_date_minus_days_text(days - 1);
    let row = client
        .query_one(MOOD_VOLATILITY_SQL.as_str(), &[&user_id, &cutoff])
        .await
        .map_err(util::db_error)?;
    let n: i64 = row.try_get("n").map_err(util::db_error)?;
    let total: Option<f64> = row.try_get("total").map_err(util::db_error)?;
    let total_squares: Option<f64> = row.try_get("total_squares").map_err(util::db_error)?;

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

/// Twin of `stats::tag_correlations`.
pub async fn tag_correlations(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<Vec<TagCorrelation>, DatabaseError> {
    let rows = client
        .query(TAG_CORRELATIONS_SQL.as_str(), &[&user_id])
        .await
        .map_err(util::db_error)?;
    rows.iter()
        .map(|row| {
            Ok(TagCorrelation {
                option_id: row.try_get("option_id").map_err(util::db_error)?,
                option_name: row.try_get("option_name").map_err(util::db_error)?,
                group_id: row.try_get("group_id").map_err(util::db_error)?,
                group_name: row.try_get("group_name").map_err(util::db_error)?,
                average_mood_selected: row
                    .try_get("average_mood_selected")
                    .map_err(util::db_error)?,
                entry_count_selected: row
                    .try_get("entry_count_selected")
                    .map_err(util::db_error)?,
                average_mood_not_selected: row
                    .try_get("average_mood_not_selected")
                    .map_err(util::db_error)?,
                entry_count_not_selected: row
                    .try_get("entry_count_not_selected")
                    .map_err(util::db_error)?,
            })
        })
        .collect()
}

/// Twin of `stats::goal_correlations`.
pub async fn goal_correlations(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<Vec<GoalCorrelation>, DatabaseError> {
    let rows = client
        .query(GOAL_CORRELATIONS_SQL.as_str(), &[&user_id])
        .await
        .map_err(util::db_error)?;
    rows.iter()
        .map(|row| {
            Ok(GoalCorrelation {
                goal_id: row.try_get("goal_id").map_err(util::db_error)?,
                goal_name: row.try_get("goal_name").map_err(util::db_error)?,
                average_mood_completed: row
                    .try_get("average_mood_completed")
                    .map_err(util::db_error)?,
                entry_count_completed: row
                    .try_get("entry_count_completed")
                    .map_err(util::db_error)?,
                average_mood_not_completed: row
                    .try_get("average_mood_not_completed")
                    .map_err(util::db_error)?,
                entry_count_not_completed: row
                    .try_get("entry_count_not_completed")
                    .map_err(util::db_error)?,
            })
        })
        .collect()
}

/// Twin of `stats::heatmap`.
pub async fn heatmap(
    client: &impl GenericClient,
    user_id: i64,
    year: i64,
) -> Result<Heatmap, DatabaseError> {
    let start = format!("{year:04}-01-01");
    let end = format!("{year:04}-12-31");
    let rows = client
        .query(HEATMAP_SQL.as_str(), &[&user_id, &start, &end])
        .await
        .map_err(util::db_error)?;
    let days = rows
        .iter()
        .map(|row| {
            Ok(HeatmapDay {
                date: row.try_get("date").map_err(util::db_error)?,
                average_mood: row.try_get("average_mood").map_err(util::db_error)?,
                entry_count: row.try_get("entry_count").map_err(util::db_error)?,
            })
        })
        .collect::<Result<Vec<_>, DatabaseError>>()?;
    let days_logged = days.len();
    Ok(Heatmap {
        year,
        days,
        days_logged,
    })
}

/// Gregorian leap-year predicate — verbatim copy of the sync module's
/// private helper (keep in sync with `db/stats.rs`; dedup candidate).
fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// `calendar.monthrange(year, month)[1]` — verbatim copy of the sync
/// module's private helper (keep in sync with `db/stats.rs`).
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

/// Twin of `stats::monthly_digest` — same composition, same verbatim
/// `ValueError` message for an out-of-range month (route callers map it
/// to HTTP 400).
pub async fn monthly_digest(
    client: &impl GenericClient,
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

    let row = client
        .query_one(DIGEST_SUMMARY_SQL.as_str(), &[&user_id, &start, &end])
        .await
        .map_err(util::db_error)?;
    let entries_logged: i64 = row.try_get("entries_logged").map_err(util::db_error)?;
    let average_mood: Option<f64> = row.try_get("average_mood").map_err(util::db_error)?;

    let row = client
        .query_one(
            DIGEST_PREVIOUS_SQL.as_str(),
            &[&user_id, &prev_start, &prev_end],
        )
        .await
        .map_err(util::db_error)?;
    let previous_average_mood: Option<f64> = row.try_get("average_mood").map_err(util::db_error)?;

    let rows = client
        .query(DIGEST_TOP_TAGS_SQL.as_str(), &[&user_id, &start, &end])
        .await
        .map_err(util::db_error)?;
    let top_tags = rows
        .iter()
        .map(|row| {
            Ok(DigestTag {
                option_id: row.try_get("option_id").map_err(util::db_error)?,
                option_name: row.try_get("option_name").map_err(util::db_error)?,
                group_name: row.try_get("group_name").map_err(util::db_error)?,
                times_selected: row.try_get("times_selected").map_err(util::db_error)?,
            })
        })
        .collect::<Result<Vec<_>, DatabaseError>>()?;

    let row = client
        .query_one(DIGEST_STREAK_SQL.as_str(), &[&user_id, &start, &end])
        .await
        .map_err(util::db_error)?;
    let longest_streak: i64 = row.try_get("longest").map_err(util::db_error)?;

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
    use super::super::util::test_support::connect_scratch;
    use super::*;
    use chrono::Datelike;
    use serde::Serialize;
    use serde_json::Value;

    /// Seed identical to the sync `stats::tests::SEED_SQL` rows (whose
    /// pinned expectations were produced by the real Python data layer):
    /// mixed ISO/US shapes, the unparseable `Aug 5, 2025`, a duplicate
    /// day, a foreign-option selection (leak check), a goal with no
    /// completions.
    const SEED_SQL: &str = "
INSERT INTO users (id, google_id, email, name) VALUES (1, 'user-one', 'one@example.com', 'One');
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

    /// User 3's volatility entries, dated relative to today exactly like
    /// the sync `seed_volatility_entries`.
    fn volatility_seed_sql() -> String {
        let today = chrono::Local::now().date_naive();
        let rows: [(i64, u64, i64, bool); 5] = [
            (301, 0, 5, true),
            (302, 1, 3, true),
            (303, 2, 4, false), // US M/D/YYYY shape, still inside the window
            (304, 5, 1, true),
            (305, 40, 5, true), // outside the 30-day window
        ];
        let mut sql = String::new();
        for (id, offset, mood, iso) in rows {
            let day = today - chrono::Days::new(offset);
            let text = if iso {
                day.format("%Y-%m-%d").to_string()
            } else {
                format!("{}/{}/{}", day.month(), day.day(), day.year())
            };
            sql.push_str(&format!(
                "INSERT INTO mood_entries (id, user_id, date, mood, content) \
                 VALUES ({id}, 3, '{text}', {mood}, 'v{id}');\n"
            ));
        }
        sql
    }

    /// The pinned JSON literals below are copied from the sync module's
    /// tests, whose values were produced by running the real Python data
    /// layer — they *are* the wire contract this twin must reproduce.
    fn assert_matches_python<T: Serialize>(actual: &T, pinned: &str) {
        let actual = serde_json::to_value(actual).unwrap();
        let expected: Value = serde_json::from_str(pinned).unwrap();
        assert_eq!(actual, expected);
    }

    /// Quirk rows exercised: US-date normalisation into the daily series,
    /// unparseable-date exclusion, EXTRACT(DOW) weekday mapping,
    /// chrono-computed volatility window, correlation CTE math with the
    /// foreign-option leak check, lexicographic year/month BETWEEN
    /// windows, and the gaps-and-islands streak.
    #[tokio::test]
    async fn pg_extended_statistics_match_python_pins() {
        let Some(client) = connect_scratch("ws4b_stats_pins").await else {
            return;
        };
        client.batch_execute(SEED_SQL).await.expect("seed");
        client
            .batch_execute(&volatility_seed_sql())
            .await
            .expect("seed volatility user");

        assert_matches_python(
            &rolling_averages(&client, 1).await.unwrap(),
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

        assert_matches_python(
            &weekday_averages(&client, 1).await.unwrap(),
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

        assert_matches_python(
            &mood_volatility(&client, 3, 30).await.unwrap(),
            r#"{"average_mood": 3.25, "entry_count": 4, "stddev": 1.707825127659933, "window_days": 30}"#,
        );
        assert_matches_python(
            &mood_volatility(&client, 1, 30).await.unwrap(),
            r#"{"average_mood": null, "entry_count": 0, "stddev": null, "window_days": 30}"#,
        );
        for days in [0, -3] {
            let err = mood_volatility(&client, 1, days).await.unwrap_err();
            assert_eq!(err.to_string(), "days must be a positive integer");
        }

        assert_matches_python(
            &tag_correlations(&client, 1).await.unwrap(),
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
        assert_matches_python(&tag_correlations(&client, 2).await.unwrap(), "[]");

        assert_matches_python(
            &goal_correlations(&client, 1).await.unwrap(),
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
        assert_matches_python(
            &goal_correlations(&client, 2).await.unwrap(),
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

        assert_matches_python(
            &heatmap(&client, 1, 2025).await.unwrap(),
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
        assert_matches_python(
            &heatmap(&client, 1, 2024).await.unwrap(),
            r#"{
              "days": [{"average_mood": 5.0, "date": "2024-12-31", "entry_count": 1}],
              "days_logged": 1,
              "year": 2024
            }"#,
        );
        assert_matches_python(
            &heatmap(&client, 2, 2025).await.unwrap(),
            r#"{
              "days": [{"average_mood": 1.0, "date": "2025-08-01", "entry_count": 1}],
              "days_logged": 1,
              "year": 2025
            }"#,
        );

        assert_matches_python(
            &monthly_digest(&client, 1, 2025, 8).await.unwrap(),
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
        assert_matches_python(
            &monthly_digest(&client, 1, 2025, 7).await.unwrap(),
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
        assert_matches_python(
            &monthly_digest(&client, 1, 2024, 12).await.unwrap(),
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
        assert_matches_python(
            &monthly_digest(&client, 1, 2025, 6).await.unwrap(),
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
        // January: the previous month rolls over into the prior year (the
        // US-format 12/31/2024 row), and a null current average keeps the
        // trend null even though the previous month has one.
        assert_matches_python(
            &monthly_digest(&client, 1, 2025, 1).await.unwrap(),
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
        for month in [0, 13, -1] {
            let err = monthly_digest(&client, 1, 2025, month).await.unwrap_err();
            assert_eq!(err.to_string(), "month must be between 1 and 12");
        }
    }

    /// Garbage-date quirk parity, pinned by running the sync SQL on
    /// SQLite (python3 sqlite3, 2026-08-22): `13/45/2025` normalises to
    /// the impossible-but-kept series point `2025-13-45`, the ISO-shaped
    /// `2025-99-99` stays a series point too, and both land in the NULL
    /// weekday bucket (strftime-on-garbage) that the shaping skips —
    /// only 2025-08-01 (a Friday) is counted.
    #[tokio::test]
    async fn pg_garbage_dates_reproduce_sqlite_quirks() {
        let Some(client) = connect_scratch("ws4b_stats_garbage").await else {
            return;
        };
        client
            .batch_execute(
                "INSERT INTO users (id, google_id, email, name) VALUES (9, 'u9', 'g@x', 'G');
                 INSERT INTO mood_entries (id, user_id, date, mood, content) VALUES
                     (901, 9, '2025-08-01', 4, 'x'),
                     (902, 9, '13/45/2025', 2, 'x'),
                     (903, 9, 'not-a-date', 5, 'x'),
                     (904, 9, '2025-99-99', 1, 'x');",
            )
            .await
            .expect("seed garbage dates");

        // Pinned from SQLite: ('2025-08-01', 4.0, 1, 4.0, 4.0),
        // ('2025-13-45', 2.0, 1, 3.0, 3.0),
        // ('2025-99-99', 1.0, 1, 2.3333333333333335, 2.3333333333333335).
        assert_matches_python(
            &rolling_averages(&client, 9).await.unwrap(),
            r#"{
              "count": 3,
              "series": [
                {"average_mood": 4.0, "date": "2025-08-01", "entry_count": 1, "rolling_30": 4.0, "rolling_7": 4.0},
                {"average_mood": 2.0, "date": "2025-13-45", "entry_count": 1, "rolling_30": 3.0, "rolling_7": 3.0},
                {"average_mood": 1.0, "date": "2025-99-99", "entry_count": 1, "rolling_30": 2.3333333333333335, "rolling_7": 2.3333333333333335}
              ]
            }"#,
        );

        // Pinned from SQLite: garbage days -> NULL weekday (skipped);
        // 2025-08-01 is weekday 5 (Friday), avg 4.0, count 1.
        assert_matches_python(
            &weekday_averages(&client, 9).await.unwrap(),
            r#"[
              {"average_mood": null, "entry_count": 0, "name": "Sunday",    "weekday": 0},
              {"average_mood": null, "entry_count": 0, "name": "Monday",    "weekday": 1},
              {"average_mood": null, "entry_count": 0, "name": "Tuesday",   "weekday": 2},
              {"average_mood": null, "entry_count": 0, "name": "Wednesday", "weekday": 3},
              {"average_mood": null, "entry_count": 0, "name": "Thursday",  "weekday": 4},
              {"average_mood": 4.0,  "entry_count": 1, "name": "Friday",    "weekday": 5},
              {"average_mood": null, "entry_count": 0, "name": "Saturday",  "weekday": 6}
            ]"#,
        );

        // The 2025 heatmap window excludes both garbage days
        // lexicographically ('2025-13-45' and '2025-99-99' sort after
        // '2025-12-31') — same as SQLite.
        assert_matches_python(
            &heatmap(&client, 9, 2025).await.unwrap(),
            r#"{
              "days": [{"average_mood": 4.0, "date": "2025-08-01", "entry_count": 1}],
              "days_logged": 1,
              "year": 2025
            }"#,
        );
    }
}
