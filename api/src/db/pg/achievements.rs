//! PostgreSQL twin of [`crate::db::achievements`] (WS4; docs/plans/v0.6.0.md
//! Feature 4, "PG wire-quirk policy").
//!
//! Quirks reproduced on purpose (the wire fixtures are the contract):
//!
//! - `first_entry_date` / `last_entry_date` are lexicographic MIN/MAX over
//!   the raw stored date strings (DECISIONS.md #4) — `date COLLATE "C"`
//!   makes Postgres MIN/MAX byte-order like SQLite BINARY, so a
//!   `not-a-date` row can still win MAX.
//! - `average_mood` is `round(x, 2) if x else 0`: a float when entries
//!   exist, the **integer** `0` otherwise (`AverageMood` keeps both JSON
//!   shapes).
//! - `nft_minted` is a raw integer 0/1 on the wire, never a boolean — the
//!   column is `integer` and the mint update writes the literal `1`
//!   (SQLite's `TRUE`).
//! - The boolean-arithmetic view-count upsert gains a `::int` cast
//!   (`(last_view_date IS NULL OR last_view_date <> $2)::int`) — the one
//!   SQL-level translation the plan calls out explicitly
//!   ([api/src/db/achievements.rs:50](api/src/db/achievements.rs#L50)).
//! - `last_insert_rowid()` sites become `RETURNING id`; the duplicate
//!   achievement insert maps integrity violations (the
//!   `sqlite3.IntegrityError` analog, [`util::is_integrity_violation`])
//!   to `None`.
//! - The streak calculator is a verbatim copy of the sync module's private
//!   parsing/walking helpers (they cannot be imported; keep the two in
//!   sync — see the dedup note in each doc comment).
//!
//! Functions follow the `pg/util.rs` conventions: `&impl GenericClient`,
//! errors via [`util::db_error`].

use std::collections::BTreeMap;

use chrono::NaiveDate;
use tokio_postgres::GenericClient;

use super::util;
use crate::db::achievements::{
    AchievementRow, AchievementsProgress, AverageMood, MoodStatistics, ProgressEntry, UserMetrics,
};
use crate::db::common::DatabaseError;

// --- SQL ---------------------------------------------------------------------

/// Twin of the sync `RECORD_STATS_VIEW_SQL` upsert. SQLite evaluates the
/// boolean subexpression to 0/1 and adds it; Postgres needs the explicit
/// `::int` cast for the same arithmetic.
const RECORD_STATS_VIEW_SQL: &str = "\
    INSERT INTO user_metrics (user_id, stats_views, last_view_date) \
    VALUES ($1, 1, $2) \
    ON CONFLICT (user_id) \
    DO UPDATE SET stats_views = user_metrics.stats_views \
                      + (user_metrics.last_view_date IS NULL \
                         OR user_metrics.last_view_date <> $2)::int, \
                  last_view_date = $2, \
                  updated_at = nightlio_now()";

const GET_LAST_VIEW_DATE_SQL: &str = "SELECT last_view_date FROM user_metrics WHERE user_id = $1";

const GET_USER_METRICS_SQL: &str =
    "SELECT user_id, stats_views, updated_at FROM user_metrics WHERE user_id = $1";

/// Twin of `SQLQueries.GET_MOOD_STATISTICS`: MIN/MAX(date) stay
/// lexicographic via the column's `COLLATE "C"` (DECISIONS.md #4).
const GET_MOOD_STATISTICS_SQL: &str = "SELECT \
      COUNT(*) as total_entries, \
      AVG(mood) as average_mood, \
      MIN(mood) as lowest_mood, \
      MAX(mood) as highest_mood, \
      MIN(date) as first_entry_date, \
      MAX(date) as last_entry_date \
    FROM mood_entries WHERE user_id = $1";

const GET_MOOD_COUNTS_SQL: &str = "SELECT mood, COUNT(*) as count \
    FROM mood_entries WHERE user_id = $1 GROUP BY mood ORDER BY mood";

/// Twin of `SQLQueries.GET_USER_ENTRY_DATES` (DISTINCT on the raw string).
const GET_USER_ENTRY_DATES_SQL: &str =
    "SELECT DISTINCT date FROM mood_entries WHERE user_id = $1 ORDER BY date DESC";

const ADD_ACHIEVEMENT_SQL: &str =
    "INSERT INTO achievements (user_id, achievement_type) VALUES ($1, $2) RETURNING id";

/// `NULLS LAST` reproduces SQLite's NULL ordering under DESC for
/// hand-edited rows with a NULL `earned_at`.
const GET_USER_ACHIEVEMENTS_SQL: &str = "SELECT id, achievement_type, earned_at, nft_minted, \
    nft_token_id, nft_tx_hash \
    FROM achievements WHERE user_id = $1 ORDER BY earned_at DESC NULLS LAST";

/// `nft_minted` is an integer column; write the literal 1 (SQLite `TRUE`)
/// so reads keep serving the raw 0/1 the fixtures pin.
const UPDATE_ACHIEVEMENT_NFT_SQL: &str = "UPDATE achievements \
    SET nft_minted = 1, nft_token_id = $1, nft_tx_hash = $2 WHERE id = $3";

// --- Statistics --------------------------------------------------------------

/// Twin of `achievements::record_stats_view` (wall-clock wrapper).
pub async fn record_stats_view(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<bool, DatabaseError> {
    record_stats_view_on(client, user_id, chrono::Local::now().date_naive()).await
}

/// Twin of `achievements::record_stats_view_on`: per-day idempotent
/// `stats_views` increment. Returns whether this call counted.
pub async fn record_stats_view_on(
    client: &impl GenericClient,
    user_id: i64,
    today: NaiveDate,
) -> Result<bool, DatabaseError> {
    let today = today.format("%Y-%m-%d").to_string();
    let prior: Option<Option<String>> = client
        .query_opt(GET_LAST_VIEW_DATE_SQL, &[&user_id])
        .await
        .map_err(util::db_error)?
        .map(|row| row.try_get(0))
        .transpose()
        .map_err(util::db_error)?;
    let counted = prior.flatten().as_deref() != Some(today.as_str());
    client
        .execute(RECORD_STATS_VIEW_SQL, &[&user_id, &today])
        .await
        .map_err(util::db_error)?;
    Ok(counted)
}

/// Twin of `achievements::get_user_metrics`. A NULL `stats_views` is
/// coerced to 0; a missing row yields the no-`updated_at` shape.
pub async fn get_user_metrics(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<UserMetrics, DatabaseError> {
    let row = client
        .query_opt(GET_USER_METRICS_SQL, &[&user_id])
        .await
        .map_err(util::db_error)?;
    match row {
        Some(row) => Ok(UserMetrics {
            user_id: row.try_get(0).map_err(util::db_error)?,
            stats_views: row
                .try_get::<_, Option<i64>>(1)
                .map_err(util::db_error)?
                .unwrap_or(0),
            updated_at: row.try_get(2).map_err(util::db_error)?,
        }),
        None => Ok(UserMetrics {
            user_id,
            stats_views: 0,
            updated_at: None,
        }),
    }
}

/// Twin of `achievements::get_mood_statistics`.
pub async fn get_mood_statistics(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<MoodStatistics, DatabaseError> {
    let row = client
        .query_one(GET_MOOD_STATISTICS_SQL, &[&user_id])
        .await
        .map_err(util::db_error)?;
    let total_entries: i64 = row.try_get(0).map_err(util::db_error)?;
    let average: Option<f64> = row.try_get(1).map_err(util::db_error)?;
    let lowest: Option<f64> = row.try_get(2).map_err(util::db_error)?;
    let highest: Option<f64> = row.try_get(3).map_err(util::db_error)?;
    let first_entry_date: Option<String> = row.try_get(4).map_err(util::db_error)?;
    let last_entry_date: Option<String> = row.try_get(5).map_err(util::db_error)?;

    if total_entries > 0 {
        // Python: `round(row[1], 2) if row[1] else 0` — falsy (None or
        // 0.0) collapses to the integer 0.
        let average_mood = match average {
            Some(x) if x != 0.0 => AverageMood::Float(python_round2(x)),
            _ => AverageMood::Int(0),
        };
        Ok(MoodStatistics {
            total_entries,
            average_mood,
            lowest_mood: util::opt_mood_value_from_f64(lowest),
            highest_mood: util::opt_mood_value_from_f64(highest),
            first_entry_date,
            last_entry_date,
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

/// Python's `round(x, 2)` (ties to even) via a format/parse round trip.
/// Verbatim copy of the sync module's private helper — keep in sync with
/// `db/achievements.rs` (dedup candidate for `pg/util.rs`).
fn python_round2(x: f64) -> f64 {
    format!("{x:.2}").parse().unwrap_or(x)
}

/// Twin of `achievements::get_mood_counts`: `{mood-key-string: count}` in
/// the BTreeMap order Flask's sorted JSON emits. Moods read as f64 fold
/// integral values back to their Python integer key form (`4` → `"4"`,
/// `4.5` → `"4.5"`).
pub async fn get_mood_counts(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<BTreeMap<String, i64>, DatabaseError> {
    let rows = client
        .query(GET_MOOD_COUNTS_SQL, &[&user_id])
        .await
        .map_err(util::db_error)?;
    let mut counts = BTreeMap::new();
    for row in rows {
        let mood: f64 = row.try_get(0).map_err(util::db_error)?;
        let count: i64 = row.try_get(1).map_err(util::db_error)?;
        counts.insert(util::mood_value_from_f64(mood).python_key_string(), count);
    }
    Ok(counts)
}

// --- Streak calculation ------------------------------------------------------

/// Twin of `achievements::get_current_streak`: any error is swallowed into
/// 0 with a warning, exactly like the Python `except Exception` guard.
pub async fn get_current_streak(client: &impl GenericClient, user_id: i64) -> i64 {
    match get_user_entry_dates(client, user_id).await {
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

async fn get_user_entry_dates(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<Vec<String>, DatabaseError> {
    let rows = client
        .query(GET_USER_ENTRY_DATES_SQL, &[&user_id])
        .await
        .map_err(util::db_error)?;
    rows.iter()
        .map(|row| row.try_get::<_, String>(0).map_err(util::db_error))
        .collect()
}

/// Verbatim copy of the sync `_parse_date_strings` port (private there;
/// keep in sync with `db/achievements.rs` — dedup candidate).
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

/// Verbatim copy of the sync `parse_python_date` (CPython `_strptime`
/// semantics: `%m/%d/%Y` first, then `%Y-%m-%d`).
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

/// Verbatim copy of the sync `_calculate_streak_from_dates` port.
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

// --- Achievements ------------------------------------------------------------

/// Twin of `achievements::add_achievement`: `None` on an integrity
/// violation (the `sqlite3.IntegrityError` analog, covering the
/// UNIQUE(user_id, achievement_type) duplicate); other errors propagate.
pub async fn add_achievement(
    client: &impl GenericClient,
    user_id: i64,
    achievement_type: &str,
) -> Result<Option<i64>, DatabaseError> {
    match client
        .query_one(ADD_ACHIEVEMENT_SQL, &[&user_id, &achievement_type])
        .await
    {
        Ok(row) => Ok(Some(row.try_get(0).map_err(util::db_error)?)),
        Err(exc) if util::is_integrity_violation(&exc) => Ok(None),
        Err(exc) => Err(util::db_error(exc)),
    }
}

/// Twin of `achievements::get_user_achievements` — rows ordered by
/// `earned_at DESC`, `nft_minted` served as the raw integer.
pub async fn get_user_achievements(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<Vec<AchievementRow>, DatabaseError> {
    let rows = client
        .query(GET_USER_ACHIEVEMENTS_SQL, &[&user_id])
        .await
        .map_err(util::db_error)?;
    rows.iter()
        .map(|row| {
            Ok(AchievementRow {
                id: row.try_get(0).map_err(util::db_error)?,
                achievement_type: row.try_get(1).map_err(util::db_error)?,
                earned_at: row.try_get(2).map_err(util::db_error)?,
                nft_minted: row
                    .try_get::<_, Option<i32>>(3)
                    .map_err(util::db_error)?
                    .map(i64::from),
                nft_token_id: row.try_get(4).map_err(util::db_error)?,
                nft_tx_hash: row.try_get(5).map_err(util::db_error)?,
            })
        })
        .collect()
}

/// Twin of `achievements::update_achievement_nft`.
pub async fn update_achievement_nft(
    client: &impl GenericClient,
    achievement_id: i64,
    token_id: i64,
    tx_hash: &str,
) -> Result<(), DatabaseError> {
    client
        .execute(
            UPDATE_ACHIEVEMENT_NFT_SQL,
            &[&token_id, &tx_hash, &achievement_id],
        )
        .await
        .map_err(util::db_error)?;
    Ok(())
}

/// Twin of `achievements::check_achievements`: awards every newly-met
/// achievement and returns the bare `achievement_type` strings in the
/// fixed check order (DECISIONS.md #8).
pub async fn check_achievements(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<Vec<String>, DatabaseError> {
    let total_entries = get_mood_statistics(client, user_id).await?.total_entries;
    let current_streak = get_current_streak(client, user_id).await;
    let stats_views = get_user_metrics(client, user_id).await?.stats_views;

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
            if let Some(id) = add_achievement(client, user_id, achievement_type).await?
                && id != 0
            {
                new_achievements.push(achievement_type.to_string());
            }
        }
    }
    Ok(new_achievements)
}

/// Twin of `achievements::get_achievements_progress` — `current` clamped
/// to `[0, max]`, fields in Flask's emitted order.
pub async fn get_achievements_progress(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<AchievementsProgress, DatabaseError> {
    let total_entries = get_mood_statistics(client, user_id).await?.total_entries;
    let current_streak = get_current_streak(client, user_id).await;
    let stats_views = get_user_metrics(client, user_id).await?.stats_views;

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
    use super::super::util::test_support::connect_scratch;
    use super::*;
    use crate::db::common::MoodValue;
    use chrono::{Datelike, Days, Local};
    use serde_json::json;

    /// Seed identical to the sync tests' user A (values pinned from the
    /// real Python data layer): fixed absolute dates, US/ISO mix, a
    /// duplicate calendar day under two raw strings, and `not-a-date`.
    async fn seed_user_a(client: &tokio_postgres::Client) {
        client
            .batch_execute(
                "INSERT INTO users (id, google_id, email, name) VALUES (1, 'userA', 'a@x', 'A');
                 INSERT INTO mood_entries (user_id, date, mood, content) VALUES
                     (1, '2025-08-01', 2, 'x'),
                     (1, '2025-12-31', 4, 'x'),
                     (1, '8/2/2025', 5, 'x'),
                     (1, '1/5/2025', 1, 'x'),
                     (1, '2025-03-15', 3, 'x'),
                     (1, '3/15/2025', 5, 'x'),
                     (1, '2025-01-02', 4, 'x'),
                     (1, 'not-a-date', 1, 'x');",
            )
            .await
            .expect("seed user A");
    }

    /// Quirk rows exercised: lexicographic MIN/MAX over raw date strings
    /// via COLLATE "C" (`not-a-date` wins MAX), Python round-half-even on
    /// the average, the mood-distribution key strings, and the empty-user
    /// integer-`0` average shape.
    #[tokio::test]
    async fn pg_statistics_lexicographic_min_max_match_python() {
        let Some(client) = connect_scratch("ws4b_ach_stats").await else {
            return;
        };
        seed_user_a(&client).await;

        // Pinned from Python: 25/8 = 3.125 -> round(3.125, 2) = 3.12 (ties
        // to even); MIN/MAX are lexicographic over raw strings, so
        // "not-a-date" ('n' > any digit) wins MAX — DECISIONS.md #4.
        let stats = get_mood_statistics(&client, 1).await.unwrap();
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

        // Pinned from Python: {1: 2, 2: 1, 3: 1, 4: 2, 5: 2}.
        assert_eq!(
            serde_json::to_value(get_mood_counts(&client, 1).await.unwrap()).unwrap(),
            json!({"1": 2, "2": 1, "3": 1, "4": 2, "5": 2})
        );

        // Schema-legal REAL mood: float MAX and the "4.5" count key.
        client
            .batch_execute(
                "INSERT INTO users (id, google_id, email, name) VALUES (5, 'userReal', 'r@x', 'R');
                 INSERT INTO mood_entries (user_id, date, mood, content) VALUES
                     (5, '2025-08-02', 4.5, 'x'),
                     (5, '2025-08-01', 2, 'x'),
                     (5, '2025-08-03', 4, 'x');",
            )
            .await
            .unwrap();
        let stats = get_mood_statistics(&client, 5).await.unwrap();
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
        assert_eq!(
            serde_json::to_value(get_mood_counts(&client, 5).await.unwrap()).unwrap(),
            json!({"2": 1, "4": 1, "4.5": 1})
        );

        // Empty user: average_mood must serialize as the INTEGER 0.
        let empty = get_mood_statistics(&client, 999).await.unwrap();
        let value = serde_json::to_value(&empty).unwrap();
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
        assert!(value["average_mood"].is_i64());
        assert!(get_mood_counts(&client, 999).await.unwrap().is_empty());
        assert_eq!(
            serde_json::to_value(get_user_metrics(&client, 999).await.unwrap()).unwrap(),
            json!({"user_id": 999, "stats_views": 0})
        );
    }

    /// Quirk rows exercised: the `::int` boolean-arithmetic upsert
    /// (per-day idempotent view counting), integrity-violation → `None`
    /// duplicate awards, and `nft_minted` as a raw wire integer.
    #[tokio::test]
    async fn pg_stats_view_upsert_and_nft_integer_match_sqlite() {
        let Some(client) = connect_scratch("ws4b_ach_upsert").await else {
            return;
        };
        client
            .batch_execute(
                "INSERT INTO users (id, google_id, email, name) VALUES (1, 'u1', 'u@x', 'U');",
            )
            .await
            .unwrap();
        let day_one = NaiveDate::parse_from_str("2020-01-01", "%Y-%m-%d").unwrap();

        // First view of the day counts; the second does not; the next day
        // counts again — the `::int` cast doing SQLite's boolean math.
        assert!(record_stats_view_on(&client, 1, day_one).await.unwrap());
        assert_eq!(get_user_metrics(&client, 1).await.unwrap().stats_views, 1);
        assert!(!record_stats_view_on(&client, 1, day_one).await.unwrap());
        assert_eq!(get_user_metrics(&client, 1).await.unwrap().stats_views, 1);
        assert!(
            record_stats_view_on(&client, 1, day_one + Days::new(1))
                .await
                .unwrap()
        );
        let metrics = get_user_metrics(&client, 1).await.unwrap();
        assert_eq!(metrics.stats_views, 2);
        assert!(metrics.updated_at.is_some());

        // The wall-clock wrapper agrees with the _on core for "today".
        let today = Local::now().date_naive();
        assert!(record_stats_view(&client, 1).await.unwrap());
        assert!(!record_stats_view_on(&client, 1, today).await.unwrap());
        assert_eq!(get_user_metrics(&client, 1).await.unwrap().stats_views, 3);

        // Duplicate award: integrity violation -> None, like
        // sqlite3.IntegrityError.
        let first = add_achievement(&client, 1, "first_entry").await.unwrap();
        assert!(matches!(first, Some(id) if id > 0));
        assert_eq!(
            add_achievement(&client, 1, "first_entry").await.unwrap(),
            None
        );

        // DB-row wire shape: nft_minted is the raw integer 0, then 1.
        let rows = get_user_achievements(&client, 1).await.unwrap();
        assert_eq!(
            serde_json::to_value(&rows[0]).unwrap()["nft_minted"],
            json!(0)
        );
        update_achievement_nft(&client, rows[0].id, 7, "0xabc")
            .await
            .unwrap();
        let row = &get_user_achievements(&client, 1).await.unwrap()[0];
        assert_eq!(row.nft_minted, Some(1));
        assert_eq!(row.nft_token_id, Some(7));
        assert_eq!(row.nft_tx_hash.as_deref(), Some("0xabc"));
        assert_eq!(
            serde_json::to_value(row).unwrap()["nft_minted"],
            json!(1),
            "nft_minted must stay a raw integer on the wire"
        );

        // earned_at DESC ordering (the awarded row's nightlio_now()
        // timestamp — today, UTC — sorts newest).
        client
            .batch_execute(
                "INSERT INTO achievements (user_id, achievement_type, earned_at) VALUES
                     (1, 'week_warrior', '2025-03-01 00:00:00'),
                     (1, 'data_lover', '2025-02-01 00:00:00');",
            )
            .await
            .unwrap();
        let types: Vec<String> = get_user_achievements(&client, 1)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.achievement_type)
            .collect();
        assert_eq!(types, ["first_entry", "week_warrior", "data_lover"]);
    }

    /// Streak + check/progress composition parity (relative dates, mixed
    /// ISO/US shapes deduping to one calendar day, check-order award
    /// list).
    #[tokio::test]
    async fn pg_streak_check_and_progress_match_python() {
        let Some(mut client) = connect_scratch("ws4b_ach_streak").await else {
            return;
        };
        client
            .batch_execute(
                "INSERT INTO users (id, google_id, email, name) VALUES (1, 'uB', 'b@x', 'B');",
            )
            .await
            .unwrap();

        let today = Local::now().date_naive();
        let us = |d: chrono::NaiveDate| format!("{}/{}/{}", d.month(), d.day(), d.year());
        let dates = [
            today.format("%Y-%m-%d").to_string(),
            us(today),
            (today - Days::new(1)).format("%Y-%m-%d").to_string(),
            us(today - Days::new(2)),
        ];
        for (date, mood) in dates.iter().zip([4, 5, 3, 2]) {
            super::super::moods::add_mood_entry(&mut client, 1, date, mood, "x", None, None)
                .await
                .unwrap();
        }

        // Pinned from Python: streak 3 (today + its US duplicate dedupe).
        assert_eq!(get_current_streak(&client, 1).await, 3);

        // check -> ["first_entry"] then [].
        assert_eq!(
            check_achievements(&client, 1).await.unwrap(),
            vec!["first_entry"]
        );
        assert_eq!(
            check_achievements(&client, 1).await.unwrap(),
            Vec::<String>::new()
        );

        // 10 distinct view days -> data_lover unlocks, in check order.
        let start = NaiveDate::parse_from_str("2026-07-01", "%Y-%m-%d").unwrap();
        for offset in 0..10 {
            assert!(
                record_stats_view_on(&client, 1, start + Days::new(offset))
                    .await
                    .unwrap()
            );
        }
        assert_eq!(
            check_achievements(&client, 1).await.unwrap(),
            vec!["data_lover"]
        );

        // Pinned from Python: progress values.
        assert_eq!(
            serde_json::to_value(get_achievements_progress(&client, 1).await.unwrap()).unwrap(),
            json!({
                "consistency_king": {"current": 3, "max": 30},
                "data_lover": {"current": 10, "max": 10},
                "first_entry": {"current": 1, "max": 1},
                "mood_master": {"current": 4, "max": 100},
                "week_warrior": {"current": 3, "max": 7}
            })
        );
    }

    // --- Pure duplicated-helper guards (no live PG needed) ----------------

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// The streak helpers are verbatim copies of the sync module's private
    /// functions; these spot checks pin the same behavior the sync tests
    /// pin so a one-sided edit gets caught.
    #[test]
    fn duplicated_streak_helpers_match_sync_semantics() {
        let raw = [
            "2026-08-15".to_string(),
            "8/15/2026".to_string(),
            "8/14/2026".to_string(),
            "not-a-date".to_string(),
            "2/29/2025".to_string(),
            "2/29/2024".to_string(),
            "4/31/2025".to_string(),
            "025-01-01".to_string(),
        ];
        assert_eq!(
            parse_date_strings(&raw),
            vec![d("2026-08-15"), d("2026-08-14"), d("2024-02-29")]
        );
        let today = d("2026-08-15");
        assert_eq!(
            calculate_streak_from_dates(
                &[
                    d("2026-08-15"),
                    d("2026-08-14"),
                    d("2026-08-13"),
                    d("2026-08-10")
                ],
                today
            ),
            3
        );
        assert_eq!(
            calculate_streak_from_dates(&[d("2026-08-13"), d("2026-08-12")], today),
            0
        );
        // Future most-recent dates still seed the walk (Python only
        // checks `(today - most_recent).days > 1`).
        assert_eq!(
            calculate_streak_from_dates(&[d("2026-08-20"), d("2026-08-19")], today),
            2
        );
        assert_eq!(parse_python_date("8/2/25"), None);
        assert_eq!(parse_python_date("2025-8-2"), Some(d("2025-08-02")));
        assert_eq!(parse_python_date("2025-08-15T00:00"), None);
    }

    #[test]
    fn duplicated_python_round2_ties_to_even() {
        assert_eq!(python_round2(3.125), 3.12);
        assert_eq!(python_round2(2.675), 2.67);
        assert_eq!(python_round2(25.0 / 8.0), 3.12);
    }
}
