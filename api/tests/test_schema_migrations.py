"""Migration tests: initialize MoodDatabase against a populated pre-Phase-2a
database and verify the forward-only migrations apply cleanly and idempotently.

Builds the OLD schema (users keyed only by google_id, groups with a global
UNIQUE(name) and no user_id, goals without last_completed_date, no
activity_log) with raw sqlite3, populates it, then runs init_database via the
MoodDatabase constructor and asserts on the result.
"""

import sqlite3

import pytest

from api.database import MoodDatabase

DEFAULT_SELF_HOST_ID = "selfhost_default_user"
FAKE_GOOGLE_ID = "108234567890123456789"

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

# Tables whose row counts must be unchanged by the migration.
PRESERVED_TABLES = [
    "users",
    "groups",
    "group_options",
    "mood_entries",
    "entry_selections",
    "goals",
    "goal_completions",
    "achievements",
]


def _build_old_populated_db(path):
    """Create a database with the pre-Phase-2a schema and realistic data."""
    conn = sqlite3.connect(path)
    conn.executescript(OLD_SCHEMA)

    conn.execute(
        "INSERT INTO users (google_id, email, name) VALUES (?, ?, ?)",
        (DEFAULT_SELF_HOST_ID, f"{DEFAULT_SELF_HOST_ID}@localhost", "Me"),
    )
    conn.execute(
        "INSERT INTO users (google_id, email, name) VALUES (?, ?, ?)",
        (FAKE_GOOGLE_ID, "google-user@example.com", "Google User"),
    )

    # All three default groups (so init seeds nothing new) plus a custom one.
    for name in ("Emotions", "Sleep", "Productivity", "Workout"):
        conn.execute("INSERT INTO groups (name) VALUES (?)", (name,))
    conn.executemany(
        "INSERT INTO group_options (group_id, name) VALUES (?, ?)",
        [(1, "happy"), (1, "sad"), (2, "well-rested"), (3, "focused"), (4, "gym")],
    )

    conn.executemany(
        "INSERT INTO mood_entries (user_id, date, mood, content) VALUES (?, ?, ?, ?)",
        [
            (1, "2026-08-01", 4, "Good day"),
            (1, "2026-08-02", 2, "Rough day"),
            (2, "2026-08-01", 5, "Great day"),
        ],
    )
    conn.executemany(
        "INSERT INTO entry_selections (entry_id, option_id) VALUES (?, ?)",
        [(1, 1), (1, 3), (2, 2), (3, 1)],
    )

    conn.execute(
        "INSERT INTO goals (user_id, title, frequency_per_week) VALUES (?, ?, ?)",
        (1, "Exercise", 3),
    )
    conn.execute(
        "INSERT INTO goal_completions (user_id, goal_id, date) VALUES (?, ?, ?)",
        (1, 1, "2026-08-01"),
    )
    conn.execute(
        "INSERT INTO achievements (user_id, achievement_type) VALUES (?, ?)",
        (1, "first_entry"),
    )

    conn.commit()
    conn.close()


def _counts(path, tables):
    conn = sqlite3.connect(path)
    try:
        return {
            t: conn.execute(f"SELECT COUNT(*) FROM {t}").fetchone()[0]  # noqa: S608
            for t in tables
        }
    finally:
        conn.close()


def _columns(path, table):
    conn = sqlite3.connect(path)
    try:
        return {row[1] for row in conn.execute(f"PRAGMA table_info({table})")}
    finally:
        conn.close()


def _schema_dump(path):
    conn = sqlite3.connect(path)
    try:
        return sorted(
            (row[0], row[1] or "")
            for row in conn.execute(
                "SELECT name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'"
            )
        )
    finally:
        conn.close()


@pytest.fixture()
def migrated_db_path(tmp_path):
    path = str(tmp_path / "old_nightlio.db")
    _build_old_populated_db(path)
    MoodDatabase(path)  # runs init_database -> migrations
    return path


def test_migration_adds_user_identity_columns(migrated_db_path):
    cols = _columns(migrated_db_path, "users")
    assert {"auth_provider", "external_id", "password_hash"} <= cols
    # Legacy column survives for backward compatibility.
    assert "google_id" in cols


def test_migration_backfills_user_identity(migrated_db_path):
    conn = sqlite3.connect(migrated_db_path)
    rows = dict(
        conn.execute("SELECT google_id, auth_provider FROM users").fetchall()
    )
    externals = dict(
        conn.execute("SELECT google_id, external_id FROM users").fetchall()
    )
    conn.close()

    assert rows[DEFAULT_SELF_HOST_ID] == "local"
    assert rows[FAKE_GOOGLE_ID] == "legacy-google"
    assert externals[DEFAULT_SELF_HOST_ID] == DEFAULT_SELF_HOST_ID
    assert externals[FAKE_GOOGLE_ID] == FAKE_GOOGLE_ID


def test_migration_preserves_default_user_and_data(migrated_db_path):
    db = MoodDatabase(migrated_db_path, init=False)
    user = db.get_user_by_google_id(DEFAULT_SELF_HOST_ID)
    assert user is not None
    assert user["id"] == 1
    entries = db.get_all_mood_entries(1)
    assert {e["date"] for e in entries} == {"2026-08-01", "2026-08-02"}


def test_migration_row_counts_unchanged(tmp_path):
    path = str(tmp_path / "counted.db")
    _build_old_populated_db(path)
    before = _counts(path, PRESERVED_TABLES)
    MoodDatabase(path)
    after = _counts(path, PRESERVED_TABLES)
    assert before == after


def test_migration_scopes_groups_to_default_user(migrated_db_path):
    conn = sqlite3.connect(migrated_db_path)
    cols = {row[1] for row in conn.execute("PRAGMA table_info(groups)")}
    assert "user_id" in cols
    owners = conn.execute("SELECT DISTINCT user_id FROM groups").fetchall()
    conn.close()
    assert owners == [(1,)]


def test_group_options_scoped_through_parent_group(migrated_db_path):
    db = MoodDatabase(migrated_db_path, init=False)
    groups = db.get_all_groups(user_id=1)
    by_name = {g["name"]: g for g in groups}
    assert "Workout" in by_name
    assert [o["name"] for o in by_name["Workout"]["options"]] == ["gym"]
    # The other user owns no groups and sees nothing.
    assert db.get_all_groups(user_id=2) == []


def test_group_name_unique_per_user_not_globally(migrated_db_path):
    db = MoodDatabase(migrated_db_path, init=False)
    # User 2 may reuse a name user 1 already has.
    new_id = db.create_group("Workout", user_id=2)
    assert new_id > 0
    # The same user may not create a duplicate.
    with pytest.raises(sqlite3.IntegrityError):
        db.create_group("Workout", user_id=2)
    with pytest.raises(sqlite3.IntegrityError):
        db.create_group("Workout", user_id=1)


def test_second_init_is_a_noop(migrated_db_path):
    schema_before = _schema_dump(migrated_db_path)
    counts_before = _counts(migrated_db_path, PRESERVED_TABLES + ["activity_log"])
    MoodDatabase(migrated_db_path)  # second init run
    assert _schema_dump(migrated_db_path) == schema_before
    assert _counts(migrated_db_path, PRESERVED_TABLES + ["activity_log"]) == counts_before


def test_fresh_and_migrated_schemas_match(tmp_path, migrated_db_path):
    fresh_path = str(tmp_path / "fresh.db")
    MoodDatabase(fresh_path)
    for table in ("users", "groups", "activity_log"):
        assert _columns(fresh_path, table) == _columns(migrated_db_path, table)


def test_provider_scoped_user_methods(migrated_db_path):
    db = MoodDatabase(migrated_db_path, init=False)

    found = db.get_user_by_provider("local", DEFAULT_SELF_HOST_ID)
    assert found is not None and found["id"] == 1

    uid = db.create_local_user("alice", "pbkdf2:fake-hash")
    assert uid > 0
    assert db.get_user_password_hash(uid) == "pbkdf2:fake-hash"
    db.set_user_password(uid, "pbkdf2:new-hash")
    assert db.get_user_password_hash(uid) == "pbkdf2:new-hash"
    with pytest.raises(sqlite3.IntegrityError):
        db.create_local_user("alice", "pbkdf2:other-hash")

    oidc1 = db.upsert_oidc_user("subject-1", "o@example.com", "Oidc User")
    oidc2 = db.upsert_oidc_user("subject-1", None, "Renamed User")
    assert oidc1 is not None and oidc2 is not None
    assert oidc1["id"] == oidc2["id"]
    assert oidc2["email"] == "o@example.com"  # COALESCE kept existing email
    assert oidc2["name"] == "Renamed User"


def test_activity_log_crud_pagination_and_prune(migrated_db_path):
    db = MoodDatabase(migrated_db_path, init=False)

    ids_u1 = [
        db.add_activity(1, "login"),
        db.add_activity(1, "entry_created", {"entry_id": 1}),
        db.add_activity(1, "entry_edited", {"entry_id": 1}),
        db.add_activity(1, "goal_completed", {"goal_id": 1}),
        db.add_activity(1, "achievement_unlocked", {"type": "first_entry"}),
    ]
    db.add_activity(2, "login")
    db.add_activity(2, "entry_created", {"entry_id": 3})

    # Newest first, per-user isolation.
    page1 = db.get_activity(1, limit=3)
    assert [r["id"] for r in page1] == ids_u1[::-1][:3]
    assert all(r["user_id"] == 1 for r in page1)
    assert page1[0]["metadata"] == {"type": "first_entry"}

    # Keyset pagination via the before cursor returns the older remainder.
    page2 = db.get_activity(1, before=page1[-1]["id"], limit=3)
    assert [r["id"] for r in page2] == ids_u1[::-1][3:]
    assert db.get_activity(1, before=page2[-1]["id"], limit=3) == []

    # The other user only sees their own two events.
    assert len(db.get_activity(2)) == 2

    # Prune removes rows older than the retention window, keeps the rest.
    old_id = db.add_activity(1, "login")
    conn = sqlite3.connect(migrated_db_path)
    conn.execute(
        "UPDATE activity_log SET created_at = '2020-01-01 00:00:00' WHERE id = ?",
        (old_id,),
    )
    conn.commit()
    conn.close()

    deleted = db.prune_activity()
    assert deleted == 1
    remaining_ids = [r["id"] for r in db.get_activity(1, limit=200)]
    assert old_id not in remaining_ids
    assert set(ids_u1) <= set(remaining_ids)
