//! `nightlio-api seed-demo` — fill a database with presentable demo data.
//!
//! Built for the self-contained demo stack (`docker-compose.demo.yml`,
//! where it runs as a one-shot service before the api starts), but equally
//! usable against a plain SQLite file. Everything goes through
//! [`crate::db::connect_from_config`] and the store facade, so it works on
//! both backends and the seeded database is byte-for-byte the database the
//! server would boot: entries are created through the same composition the
//! `POST /mood` route uses (activity log + achievement checks included, so
//! achievements unlock naturally), goals through the goals store with
//! backdated completions.
//!
//! Exit-code contract (`main.rs` maps these): success AND the
//! already-seeded no-op both exit 0 — the demo compose gates the api on
//! `service_completed_successfully`, and a restarted stack must come up —
//! failures exit 1, usage errors exit 2.

use std::collections::HashMap;

use anyhow::Context;

use crate::config::Config;
use crate::db::store;

pub const USAGE: &str = "usage: nightlio-api seed-demo [--database-url <url>] [--sqlite <path>] [--force]\n\
     defaults: the backend the server would use (DATABASE_URL if set, else DATABASE_PATH)";

/// Parsed `seed-demo` invocation: the (possibly flag-overridden) config to
/// boot with, plus the `--force` switch.
pub struct Options {
    pub config: Config,
    pub force: bool,
}

/// No-clap argument parsing (the `migrate-to-postgres` convention). Errors
/// are user-facing strings carrying the usage line; the caller exits 2.
pub fn parse_args(cfg: &Config, args: &[String]) -> Result<Options, String> {
    let mut database_url: Option<String> = None;
    let mut sqlite_path: Option<String> = None;
    let mut force = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--database-url" => {
                database_url = Some(
                    iter.next()
                        .cloned()
                        .ok_or_else(|| format!("--database-url needs a value\n{USAGE}"))?,
                );
            }
            "--sqlite" => {
                sqlite_path = Some(
                    iter.next()
                        .cloned()
                        .ok_or_else(|| format!("--sqlite needs a value\n{USAGE}"))?,
                );
            }
            "--force" => force = true,
            other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
        }
    }
    if database_url.is_some() && sqlite_path.is_some() {
        return Err(format!(
            "--database-url and --sqlite are mutually exclusive (one target per run)\n{USAGE}"
        ));
    }

    let mut config = cfg.clone();
    if let Some(url) = database_url {
        config.database_url = Some(url);
    }
    if let Some(path) = sqlite_path {
        // An explicit --sqlite means "seed this file": it overrides any
        // DATABASE_URL the environment may carry.
        config.database_url = None;
        config.database_path = path;
    }
    Ok(Options { config, force })
}

/// What a `seed-demo` run did (both variants exit 0).
pub enum SeedOutcome {
    /// The database already holds entries and `--force` was not passed.
    AlreadySeeded {
        entries: usize,
    },
    Seeded(SeedSummary),
}

/// Counts printed after a successful seed.
pub struct SeedSummary {
    pub user_id: i64,
    pub entries: usize,
    pub goals: usize,
    pub goal_completions: usize,
    pub achievements: usize,
}

impl SeedOutcome {
    /// The human-readable report `main.rs` prints on exit 0.
    pub fn report(&self) -> String {
        match self {
            SeedOutcome::AlreadySeeded { entries } => format!(
                "already seeded ({entries} entries); pass --force to add demo data anyway\n"
            ),
            SeedOutcome::Seeded(summary) => format!(
                "seed-demo complete for user {}:\n\
                 \x20 mood entries:     {}\n\
                 \x20 goals:            {}\n\
                 \x20 goal completions: {}\n\
                 \x20 achievements:     {}\n",
                summary.user_id,
                summary.entries,
                summary.goals,
                summary.goal_completions,
                summary.achievements
            ),
        }
    }
}

// --- The dataset -------------------------------------------------------------
//
// Fourteen markdown entries over the last fourteen days, in the voice of the
// README captures (`e2e/readme-captures.spec.ts` — the first seven are those
// exact texts); a mood mix spanning 1–5; one to three tag selections each,
// matched to the entry's mood and drawn from the default groups.

struct DemoEntry {
    days_ago: i64,
    mood: i64,
    content: &'static str,
    /// `(group name, option name)` pairs from the default groups.
    tags: &'static [(&'static str, &'static str)],
}

const DEMO_ENTRIES: &[DemoEntry] = &[
    DemoEntry {
        days_ago: 0,
        mood: 5,
        content: "# Shipped the rewrite\nEverything green on the first run. Celebrated with a long walk and *way* too much coffee.",
        tags: &[
            ("Emotions", "happy"),
            ("Emotions", "excited"),
            ("Productivity", "accomplished"),
        ],
    },
    DemoEntry {
        days_ago: 1,
        mood: 4,
        content: "# Quiet focus day\nDeep work most of the morning, gym in the evening.\n\n- finished the migration notes\n- 5k on the treadmill",
        tags: &[("Productivity", "focused"), ("Productivity", "motivated")],
    },
    DemoEntry {
        days_ago: 2,
        mood: 3,
        content: "# Middling\nSlept badly, but the afternoon picked up after a proper lunch break.",
        tags: &[("Sleep", "restless"), ("Emotions", "unsure")],
    },
    DemoEntry {
        days_ago: 3,
        mood: 4,
        content: "# Board-game night\nLost twice at Wingspan, still worth it.",
        tags: &[("Emotions", "content"), ("Emotions", "grateful")],
    },
    DemoEntry {
        days_ago: 4,
        mood: 2,
        content: "# Rough one\nServer alerts at 3am. Wrote down what to automate so it never pages me again.",
        tags: &[
            ("Emotions", "stressed"),
            ("Sleep", "exhausted"),
            ("Productivity", "overwhelmed"),
        ],
    },
    DemoEntry {
        days_ago: 5,
        mood: 4,
        content: "# Recovery\nSlow morning, long reading session, early night.",
        tags: &[("Emotions", "relaxed"), ("Sleep", "refreshed")],
    },
    DemoEntry {
        days_ago: 6,
        mood: 5,
        content: "# Hike day\nTwelve kilometres of forest trail and zero notifications.",
        tags: &[
            ("Emotions", "happy"),
            ("Emotions", "grateful"),
            ("Sleep", "well-rested"),
        ],
    },
    DemoEntry {
        days_ago: 7,
        mood: 3,
        content: "# Meetings all day\nBack-to-back calls until four. Salvaged the evening with a slow ramen recipe.",
        tags: &[("Productivity", "busy"), ("Emotions", "tired")],
    },
    DemoEntry {
        days_ago: 8,
        mood: 1,
        content: "# Flat\nNothing specific went wrong; the day just never started. Went to bed early instead of forcing it.",
        tags: &[("Emotions", "sad"), ("Sleep", "insomniac")],
    },
    DemoEntry {
        days_ago: 9,
        mood: 4,
        content: "# Small wins\nInbox zero, a tidy desk, and the bug that haunted me all week fell in ten minutes.\n\n- closed 4 tickets\n- meal-prepped for two days",
        tags: &[("Productivity", "accomplished"), ("Emotions", "content")],
    },
    DemoEntry {
        days_ago: 10,
        mood: 2,
        content: "# Deadline dread\nSpent more time worrying about the review than the review took. Note to self: start earlier.",
        tags: &[("Emotions", "anxious"), ("Productivity", "procrastinating")],
    },
    DemoEntry {
        days_ago: 11,
        mood: 5,
        content: "# Old friends\nDinner ran three hours past the reservation and nobody noticed. Home late, heart full.",
        tags: &[("Emotions", "happy"), ("Emotions", "grateful")],
    },
    DemoEntry {
        days_ago: 12,
        mood: 3,
        content: "# Autopilot\nGroceries, laundry, a run in the drizzle. Unremarkable and somehow exactly what was needed.",
        tags: &[("Emotions", "bored"), ("Sleep", "tired")],
    },
    DemoEntry {
        days_ago: 13,
        mood: 4,
        content: "# Library morning\nTwo chapters and a pot of tea before noon. The afternoon could not keep up, but it tried.",
        tags: &[("Emotions", "relaxed"), ("Productivity", "focused")],
    },
];

/// The README-capture goal set, each with two to four backdated completion
/// days (distinct per goal, so every increment logs a new completion).
struct DemoGoal {
    title: &'static str,
    description: &'static str,
    frequency: i64,
    completions_days_ago: &'static [i64],
}

const DEMO_GOALS: &[DemoGoal] = &[
    DemoGoal {
        title: "Train 3x a week",
        description: "Any workout counts.",
        frequency: 3,
        completions_days_ago: &[1, 3, 6],
    },
    DemoGoal {
        title: "Read before bed",
        description: "Twenty minutes, no phone.",
        frequency: 5,
        completions_days_ago: &[0, 2, 5, 9],
    },
    DemoGoal {
        title: "Cook at home",
        description: "Takeout is for Fridays.",
        frequency: 4,
        completions_days_ago: &[0, 1, 4],
    },
    DemoGoal {
        title: "Morning pages",
        description: "Three sentences before coffee.",
        frequency: 7,
        completions_days_ago: &[2, 3, 7, 10],
    },
    DemoGoal {
        title: "Walk outside",
        description: "Daylight before noon.",
        frequency: 5,
        completions_days_ago: &[0, 6],
    },
    DemoGoal {
        title: "Call someone",
        description: "Family or an old friend.",
        frequency: 2,
        completions_days_ago: &[4, 11],
    },
];

/// Local calendar date `days` days ago, ISO-formatted — the same date math
/// the app itself uses for "today".
fn iso_days_ago(days: i64) -> String {
    (chrono::Local::now().date_naive() - chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string()
}

// --- The seed run ------------------------------------------------------------

/// Boot the configured backend through the production startup path and
/// seed the demo dataset for the self-host user. Idempotent by default: a
/// database that already holds mood entries becomes a no-op (exit 0) unless
/// `force` is set.
pub async fn run(cfg: &Config, force: bool) -> anyhow::Result<SeedOutcome> {
    let db = crate::db::connect_from_config(cfg).await?;

    // The self-host user, exactly as the credential-free login branch
    // upserts it (routes/auth.rs): name from config, email falling back to
    // `<id>@localhost`.
    let default_user_id = cfg.default_self_host_id.clone();
    let name = cfg.selfhost_user_name.clone();
    let email = cfg
        .selfhost_user_email
        .clone()
        .unwrap_or_else(|| format!("{default_user_id}@localhost"));
    let user = store::users::selfhost_login_upsert(&db, default_user_id, email, name)
        .await
        .context("upserting the self-host user")?
        .context("self-host user upsert returned no row")?;
    let user_id = user.id;

    // Idempotency gate: any existing mood entries mean the database is in
    // use (or already seeded) — leave it alone unless --force.
    let existing = store::moods::get_entries(&db, user_id, None, None)
        .await
        .map_err(|exc| anyhow::anyhow!(exc).context("listing existing entries"))?;
    if !existing.is_empty() && !force {
        return Ok(SeedOutcome::AlreadySeeded {
            entries: existing.len(),
        });
    }

    // Resolve the default-group option names to ids (they exist on both
    // backends via the startup baseline). Missing names — a user-modified
    // database under --force — are simply skipped.
    let groups = store::groups::get_all_groups(&db, user_id, cfg.default_self_host_id.clone())
        .await
        .map_err(|exc| anyhow::anyhow!(exc).context("listing tag groups"))?;
    let mut option_ids: HashMap<(String, String), i64> = HashMap::new();
    for group in &groups {
        for option in &group.options {
            option_ids.insert((group.name.clone(), option.name.clone()), option.id);
        }
    }

    // Entries, through the same store composition as POST /mood — activity
    // log and achievement checks included, so achievements unlock naturally.
    for entry in DEMO_ENTRIES {
        let selected: Vec<i64> = entry
            .tags
            .iter()
            .filter_map(|(group, option)| {
                option_ids
                    .get(&((*group).to_string(), (*option).to_string()))
                    .copied()
            })
            .collect();
        store::moods::create_entry(
            &db,
            user_id,
            iso_days_ago(entry.days_ago),
            entry.mood,
            entry.content.to_string(),
            None,
            selected,
        )
        .await
        .map_err(|exc| {
            anyhow::anyhow!(exc).context(format!("seeding entry {:?}", entry.days_ago))
        })?;
    }

    // Goals with backdated completions (the goal-backdating store path).
    let mut goal_completions = 0usize;
    for goal in DEMO_GOALS {
        let goal_id = store::goals::create_goal(
            &db,
            user_id,
            goal.title.to_string(),
            goal.description.to_string(),
            goal.frequency,
        )
        .await
        .map_err(|exc| anyhow::anyhow!(exc).context(format!("seeding goal {:?}", goal.title)))?;
        for days_ago in goal.completions_days_ago {
            let progress = store::goals::increment_progress(
                &db,
                user_id,
                goal_id,
                Some(iso_days_ago(*days_ago)),
            )
            .await
            .map_err(|exc| {
                anyhow::anyhow!(exc).context(format!("logging progress for goal {:?}", goal.title))
            })?;
            anyhow::ensure!(
                progress.is_some(),
                "goal {} vanished while seeding completions",
                goal_id
            );
            goal_completions += 1;
        }
    }

    // One counted statistics view, so the stats-related progress meters are
    // not all at zero.
    store::achievements::record_stats_view(&db, user_id)
        .await
        .map_err(|exc| anyhow::anyhow!(exc).context("recording a statistics view"))?;

    let achievements = store::achievements::get_user_achievements(&db, user_id)
        .await
        .map_err(|exc| anyhow::anyhow!(exc).context("counting achievements"))?
        .len();

    Ok(SeedOutcome::Seeded(SeedSummary {
        user_id,
        entries: DEMO_ENTRIES.len(),
        goals: DEMO_GOALS.len(),
        goal_completions,
        achievements,
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn dev_config(dir: &tempfile::TempDir) -> Config {
        let vars: HashMap<String, String> =
            HashMap::from([("APP_ENV".to_string(), "development".to_string())]);
        let lookup = move |key: &str| vars.get(key).cloned();
        let mut cfg = Config::from_lookup(&lookup);
        cfg.database_path = dir
            .path()
            .join("nightlio.db")
            .to_string_lossy()
            .into_owned();
        cfg
    }

    #[test]
    fn dataset_shape_is_the_documented_one() {
        assert_eq!(
            DEMO_ENTRIES.len(),
            14,
            "fourteen entries over fourteen days"
        );
        let offsets: Vec<i64> = DEMO_ENTRIES.iter().map(|e| e.days_ago).collect();
        assert_eq!(offsets, (0..14).collect::<Vec<_>>(), "one entry per day");
        for mood in 1..=5 {
            assert!(
                DEMO_ENTRIES.iter().any(|e| e.mood == mood),
                "mood mix must span 1–5 (missing {mood})"
            );
        }
        for entry in DEMO_ENTRIES {
            assert!(
                (1..=3).contains(&entry.tags.len()),
                "1–3 selections per entry"
            );
        }
        assert_eq!(DEMO_GOALS.len(), 6);
        for goal in DEMO_GOALS {
            assert!(
                (2..=4).contains(&goal.completions_days_ago.len()),
                "2–4 completions per goal"
            );
            let mut days = goal.completions_days_ago.to_vec();
            days.dedup();
            assert_eq!(
                days.len(),
                goal.completions_days_ago.len(),
                "completion days must be distinct per goal"
            );
        }
    }

    /// Every tag in the dataset must resolve against the default groups —
    /// a typo here would silently seed an entry with fewer selections.
    #[test]
    fn every_tag_resolves_against_the_default_groups() {
        use crate::db::bootstrap::DEFAULT_GROUPS;
        for entry in DEMO_ENTRIES {
            for (group, option) in entry.tags {
                let found = DEFAULT_GROUPS
                    .iter()
                    .any(|(name, options)| name == group && options.contains(option));
                assert!(found, "unknown tag ({group}, {option})");
            }
        }
    }

    #[tokio::test]
    async fn sqlite_run_seeds_then_noops_then_force_appends() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dev_config(&dir);

        let outcome = run(&cfg, false).await.unwrap();
        let SeedOutcome::Seeded(summary) = outcome else {
            panic!("first run must seed");
        };
        assert_eq!(summary.entries, 14);
        assert_eq!(summary.goals, 6);
        assert_eq!(summary.goal_completions, 18);
        assert!(summary.achievements >= 1, "achievements unlock naturally");

        // Second run: already-seeded no-op, still Ok (exit 0 in main).
        let outcome = run(&cfg, false).await.unwrap();
        let SeedOutcome::AlreadySeeded { entries } = outcome else {
            panic!("second run must be a no-op");
        };
        assert_eq!(entries, 14);
        assert!(
            outcome
                .report()
                .contains("already seeded (14 entries); pass --force to add demo data anyway")
        );

        // --force appends another dataset.
        let outcome = run(&cfg, true).await.unwrap();
        assert!(matches!(outcome, SeedOutcome::Seeded(_)));
        let conn = rusqlite::Connection::open(dir.path().join("nightlio.db")).unwrap();
        let entries: i64 = conn
            .query_row("SELECT COUNT(*) FROM mood_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(entries, 28);
    }
}
