"""Build the migration-test DB corpus under contract/corpus/.

Produces:
  fresh.db                  - init_database on an empty path (current schema)
  legacy-groups.db          - pre-Phase-2a schema (no groups.user_id, global
                              UNIQUE(name)), populated; NOT migrated
  half-migrated.db          - current schema minus users.theme_preference and
                              minus idx_users_provider_external, populated
  legacy-groups.migrated.db - copy of legacy-groups.db after Python bootstrap
  half-migrated.migrated.db - copy of half-migrated.db after Python bootstrap

For every DB: <name>.pragmas.txt (PRAGMA table_info + index_list [+ index_info]
per table) and <name>.counts.txt (per-table row counts).
"""

import os
import shutil
import sqlite3
import sys

# contract/corpus/build_corpus.py -> repo root is three levels up.
REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CORPUS = os.path.join(REPO, "contract", "corpus")

sys.path.insert(0, REPO)
from api.database import MoodDatabase  # noqa: E402

DEFAULT_SELF_HOST_ID = "selfhost_default_user"
FAKE_GOOGLE_ID = "108234567890123456789"

# Pre-Phase-2a schema, verbatim from api/tests/test_schema_migrations.py.
OLD_SCHEMA = """
CREATE TABLE users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    google_id TEXT UNIQUE NOT NULL,
    email TEXT NOT NULL,
    name TEXT NOT NULL,
    avatar_url TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_login TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE mood_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    date TEXT NOT NULL,
    mood INTEGER NOT NULL CHECK (mood >= 1 AND mood <= 5),
    content TEXT NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
);
CREATE TABLE groups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE group_options (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (group_id) REFERENCES groups (id) ON DELETE CASCADE
);
CREATE TABLE entry_selections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entry_id INTEGER NOT NULL,
    option_id INTEGER NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (entry_id) REFERENCES mood_entries (id) ON DELETE CASCADE,
    FOREIGN KEY (option_id) REFERENCES group_options (id) ON DELETE CASCADE
);
CREATE TABLE achievements (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    achievement_type TEXT NOT NULL,
    earned_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    nft_minted BOOLEAN DEFAULT FALSE,
    nft_token_id INTEGER,
    nft_tx_hash TEXT,
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    UNIQUE(user_id, achievement_type)
);
CREATE TABLE goals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    description TEXT,
    frequency_per_week INTEGER NOT NULL CHECK (frequency_per_week >= 1 AND frequency_per_week <= 7),
    completed INTEGER NOT NULL DEFAULT 0,
    streak INTEGER NOT NULL DEFAULT 0,
    period_start TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
);
CREATE TABLE goal_completions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    goal_id INTEGER NOT NULL,
    date TEXT NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    FOREIGN KEY (goal_id) REFERENCES goals (id) ON DELETE CASCADE,
    UNIQUE(user_id, goal_id, date)
);
CREATE TABLE user_metrics (
    user_id INTEGER PRIMARY KEY,
    stats_views INTEGER NOT NULL DEFAULT 0,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
);
CREATE INDEX idx_mood_entries_date ON mood_entries(date);
"""

# Fixed timestamp so regeneration is stable where we control inserts.
TS = "2026-08-15 00:00:00"


def build_legacy_groups_db(path):
    """Pre-Phase-2a schema, populated (mirrors _build_old_populated_db)."""
    conn = sqlite3.connect(path)
    conn.executescript(OLD_SCHEMA)

    conn.executemany(
        "INSERT INTO users (google_id, email, name, created_at, last_login) "
        "VALUES (?, ?, ?, ?, ?)",
        [
            (DEFAULT_SELF_HOST_ID, f"{DEFAULT_SELF_HOST_ID}@localhost", "Me", TS, TS),
            (FAKE_GOOGLE_ID, "google-user@example.com", "Google User", TS, TS),
        ],
    )

    # All three default groups (so init seeds nothing new) plus a custom one.
    for name in ("Emotions", "Sleep", "Productivity", "Workout"):
        conn.execute(
            "INSERT INTO groups (name, created_at) VALUES (?, ?)", (name, TS)
        )
    conn.executemany(
        "INSERT INTO group_options (group_id, name, created_at) VALUES (?, ?, ?)",
        [
            (1, "happy", TS),
            (1, "sad", TS),
            (2, "well-rested", TS),
            (3, "focused", TS),
            (4, "gym", TS),
        ],
    )

    conn.executemany(
        "INSERT INTO mood_entries (user_id, date, mood, content, created_at, updated_at) "
        "VALUES (?, ?, ?, ?, ?, ?)",
        [
            (1, "2026-08-01", 4, "Good day", TS, TS),
            (1, "2026-08-02", 2, "Rough day", TS, TS),
            (2, "2026-08-01", 5, "Great day", TS, TS),
        ],
    )
    conn.executemany(
        "INSERT INTO entry_selections (entry_id, option_id, created_at) VALUES (?, ?, ?)",
        [(1, 1, TS), (1, 3, TS), (2, 2, TS), (3, 1, TS)],
    )

    conn.execute(
        "INSERT INTO goals (user_id, title, frequency_per_week, created_at, updated_at) "
        "VALUES (?, ?, ?, ?, ?)",
        (1, "Exercise", 3, TS, TS),
    )
    conn.execute(
        "INSERT INTO goal_completions (user_id, goal_id, date, created_at) "
        "VALUES (?, ?, ?, ?)",
        (1, 1, "2026-08-01", TS),
    )
    conn.execute(
        "INSERT INTO achievements (user_id, achievement_type, earned_at) "
        "VALUES (?, ?, ?)",
        (1, "first_entry", TS),
    )
    conn.execute(
        "INSERT INTO user_metrics (user_id, stats_views, updated_at) VALUES (?, ?, ?)",
        (1, 7, TS),
    )

    conn.commit()
    conn.close()


def build_half_migrated_db(path):
    """Current schema minus theme_preference / idx_users_provider_external,
    with rows, simulating a production DB migrated by an older build."""
    MoodDatabase(path)  # full current schema + seeded user + default groups

    conn = sqlite3.connect(path)

    # A legacy Google row written by old code: identity columns left NULL so
    # the re-run bootstrap must backfill auth_provider/external_id.
    conn.execute(
        "INSERT INTO users (google_id, email, name, created_at, last_login) "
        "VALUES (?, ?, ?, ?, ?)",
        (FAKE_GOOGLE_ID, "google-user@example.com", "Google User", TS, TS),
    )

    conn.executemany(
        "INSERT INTO mood_entries (user_id, date, mood, content, created_at, updated_at) "
        "VALUES (?, ?, ?, ?, ?, ?)",
        [
            (1, "2026-08-01", 4, "Good day", TS, TS),
            (1, "2026-08-02", 2, "Rough day", TS, TS),
            (2, "2026-08-01", 5, "Great day", TS, TS),
        ],
    )
    # Option ids reference the seeded default groups (1..27 exist).
    conn.executemany(
        "INSERT INTO entry_selections (entry_id, option_id, created_at) VALUES (?, ?, ?)",
        [(1, 1, TS), (1, 14, TS), (2, 2, TS), (3, 1, TS)],
    )
    conn.execute(
        "INSERT INTO goals (user_id, title, description, frequency_per_week, "
        "completed, streak, period_start, last_completed_date, created_at, updated_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (1, "Exercise", "Move more", 3, 1, 2, "2026-07-28", "2026-08-01", TS, TS),
    )
    conn.execute(
        "INSERT INTO goal_completions (user_id, goal_id, date, created_at) "
        "VALUES (?, ?, ?, ?)",
        (1, 1, "2026-08-01", TS),
    )
    conn.execute(
        "INSERT INTO achievements (user_id, achievement_type, earned_at) "
        "VALUES (?, ?, ?)",
        (1, "first_entry", TS),
    )
    conn.execute(
        "INSERT INTO user_metrics (user_id, stats_views, updated_at) VALUES (?, ?, ?)",
        (1, 3, TS),
    )
    conn.executemany(
        "INSERT INTO activity_log (user_id, event_type, metadata, created_at) "
        "VALUES (?, ?, ?, ?)",
        [
            (1, "login", None, TS),
            (1, "entry_created", '{"entry_id": 1}', TS),
            (2, "login", None, TS),
        ],
    )
    conn.commit()

    # Now un-migrate: drop the provider-identity index and theme_preference.
    conn.execute("DROP INDEX idx_users_provider_external")
    conn.execute("ALTER TABLE users DROP COLUMN theme_preference")
    conn.commit()
    conn.close()


def dump_pragmas(db_path, out_path):
    conn = sqlite3.connect(db_path)
    tables = [
        r[0]
        for r in conn.execute(
            "SELECT name FROM sqlite_master "
            "WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        )
    ]
    lines = []
    for t in tables:
        lines.append(f"== table: {t}")
        lines.append("-- PRAGMA table_info (cid, name, type, notnull, dflt_value, pk)")
        for row in conn.execute(f'PRAGMA table_info("{t}")'):
            lines.append("   " + repr(tuple(row)))
        lines.append("-- PRAGMA index_list (seq, name, unique, origin, partial)")
        index_rows = list(conn.execute(f'PRAGMA index_list("{t}")'))
        for row in index_rows:
            lines.append("   " + repr(tuple(row)))
        for row in index_rows:
            idx = row[1].replace('"', '""')
            cols = [r[2] for r in conn.execute(f'PRAGMA index_info("{idx}")')]
            lines.append(f"-- PRAGMA index_info({row[1]}) columns: {cols!r}")
        lines.append("")
    conn.close()
    with open(out_path, "w") as f:
        f.write("\n".join(lines))


def dump_counts(db_path, out_path):
    conn = sqlite3.connect(db_path)
    tables = [
        r[0]
        for r in conn.execute(
            "SELECT name FROM sqlite_master "
            "WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        )
    ]
    lines = []
    for t in tables:
        n = conn.execute(f'SELECT COUNT(*) FROM "{t}"').fetchone()[0]
        lines.append(f"{t}\t{n}")
    conn.close()
    with open(out_path, "w") as f:
        f.write("\n".join(lines) + "\n")


def dump_all(name):
    db = os.path.join(CORPUS, f"{name}.db")
    dump_pragmas(db, os.path.join(CORPUS, f"{name}.pragmas.txt"))
    dump_counts(db, os.path.join(CORPUS, f"{name}.counts.txt"))


def main():
    os.makedirs(CORPUS, exist_ok=True)
    # Remove only generated files; keep README.md and this script.
    for fn in os.listdir(CORPUS):
        if fn.endswith((".db", ".pragmas.txt", ".counts.txt")):
            os.remove(os.path.join(CORPUS, fn))

    # 1. fresh.db
    MoodDatabase(os.path.join(CORPUS, "fresh.db"))
    dump_all("fresh")

    # 2. legacy-groups.db (NOT migrated)
    build_legacy_groups_db(os.path.join(CORPUS, "legacy-groups.db"))
    dump_all("legacy-groups")

    # 3. half-migrated.db
    build_half_migrated_db(os.path.join(CORPUS, "half-migrated.db"))
    dump_all("half-migrated")

    # 4. expected post-migration outputs
    for name in ("legacy-groups", "half-migrated"):
        src = os.path.join(CORPUS, f"{name}.db")
        dst = os.path.join(CORPUS, f"{name}.migrated.db")
        shutil.copyfile(src, dst)
        MoodDatabase(dst)  # Python bootstrap = expected migration output
        dump_all(f"{name}.migrated")

    print("corpus built OK")


if __name__ == "__main__":
    main()
