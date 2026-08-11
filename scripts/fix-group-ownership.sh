#!/usr/bin/env bash
# One-time repair: give the original tag groups back to the real user.
#
# The per-user groups migration backfilled the legacy shared groups
# (Emotions / Sleep / Productivity, ids 1-3) to the seeded self-host user
# (id 4, "Me") instead of the account that actually owns the entries and
# selections (id 1). Result: the entry editor shows no categories.
#
# This script, inside the api container:
#   1. backs up the DB file next to itself,
#   2. moves groups 1-3 from user 4 to user 1,
#   3. seeds the default groups for the other affected user (id 3) so their
#      editor is not empty either (their historical selections keep
#      resolving by name via the unscoped entry-detail join).
#
# Idempotent: re-running moves nothing and seeds nothing new.

set -euo pipefail

CONTAINER="${NIGHTLIO_API_CONTAINER:-nightlio-api}"

docker exec -i "$CONTAINER" python - <<'PY'
import os, sqlite3, shutil, datetime

path = os.environ.get("DATABASE_PATH", "/app/data/nightlio.db")
stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
backup = f"{path}.bak-{stamp}-groupfix"
shutil.copy2(path, backup)
print(f"backup: {backup}")

DEFAULT_GROUPS = {
    "Emotions": ["happy", "excited", "grateful", "relaxed", "content", "tired",
                 "unsure", "bored", "anxious", "angry", "stressed", "sad",
                 "desperate"],
    "Sleep": ["well-rested", "refreshed", "tired", "exhausted", "restless",
              "insomniac"],
    "Productivity": ["focused", "motivated", "accomplished", "productive",
                     "procrastinating", "distracted", "overwhelmed", "lazy"],
}

db = sqlite3.connect(path)
db.execute("PRAGMA foreign_keys=ON")
db.execute("BEGIN IMMEDIATE")
try:
    n = db.execute(
        "UPDATE groups SET user_id = 1 WHERE id IN (1, 2, 3) AND user_id = 4"
    ).rowcount
    print(f"groups moved to user 1: {n}")

    for gname, options in DEFAULT_GROUPS.items():
        row = db.execute(
            "SELECT id FROM groups WHERE name = ? AND user_id = 3", (gname,)
        ).fetchone()
        if not row:
            gid = db.execute(
                "INSERT INTO groups (name, user_id) VALUES (?, 3)", (gname,)
            ).lastrowid
            db.executemany(
                "INSERT INTO group_options (group_id, name) VALUES (?, ?)",
                [(gid, o) for o in options],
            )
            print(f"seeded '{gname}' for user 3")

    db.commit()
except Exception:
    db.rollback()
    raise

print("final state:")
for r in db.execute("SELECT id, user_id, name FROM groups ORDER BY user_id, id"):
    print(f"  group {r[0]} user {r[1]} {r[2]}")
PY
