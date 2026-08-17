//! Ad hoc, ignored-by-default harness for Phase 2 prod-corpus validation
//! (see contract/corpus/README.md procedure). Not part of the regular
//! suite: runs `bootstrap()` against a path supplied via
//! `MANUAL_BOOTSTRAP_DB_PATH` so the migration can be driven against
//! scratch copies of the real production DBs living outside this repo's
//! tracked corpus. Safe to delete once that validation pass is done.

use nightlio_api::db::achievements::get_mood_statistics;
use nightlio_api::db::bootstrap::{SelfHostSeed, bootstrap};
use nightlio_api::db::moods::get_mood_entries_by_date_range;
use rusqlite::Connection;

#[test]
#[ignore]
fn manual_bootstrap_from_env() {
    let path = std::env::var("MANUAL_BOOTSTRAP_DB_PATH")
        .expect("set MANUAL_BOOTSTRAP_DB_PATH to the scratch copy to bootstrap");
    bootstrap(&path, &SelfHostSeed::default()).expect("bootstrap failed");
}

/// Ad hoc: after `manual_bootstrap_from_env` has normalized a scratch copy
/// of a real production DB, exercises the exact same in-process code paths
/// the `/api/statistics` route and the mood date-range query use — no HTTP
/// layer, no auth — against a user id supplied via `MANUAL_STATS_USER_ID`.
/// Prints the statistics struct and the ids returned by a date range
/// spanning the formerly-US-format rows, so the date-normalization contract fix
/// can be eyeballed against a real corpus DB. Not part of the regular
/// suite; safe to delete once that validation pass is done.
#[test]
#[ignore]
fn manual_statistics_for_user() {
    let path = std::env::var("MANUAL_BOOTSTRAP_DB_PATH")
        .expect("set MANUAL_BOOTSTRAP_DB_PATH to the scratch copy to inspect");
    let user_id: i64 = std::env::var("MANUAL_STATS_USER_ID")
        .expect("set MANUAL_STATS_USER_ID")
        .parse()
        .expect("MANUAL_STATS_USER_ID must be an integer");
    let conn = Connection::open(&path).expect("open scratch copy");

    let stats = get_mood_statistics(&conn, user_id).expect("get_mood_statistics");
    println!("STATS={stats:?}");

    // A range spanning the formerly-US-format June/July rows plus the
    // native-ISO August row, expressed in ISO (post-normalization the raw
    // BETWEEN comparison this route uses only works in ISO).
    let range = get_mood_entries_by_date_range(&conn, user_id, "2026-06-01", "2026-08-31")
        .expect("get_mood_entries_by_date_range");
    println!("RANGE_COUNT={}", range.len());
    for entry in &range {
        println!("RANGE_ROW id={} date={}", entry.id, entry.date);
    }
}
