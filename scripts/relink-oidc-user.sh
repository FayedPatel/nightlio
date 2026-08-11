#!/usr/bin/env bash
# Re-attach an existing Nightlio account to a new OIDC identity.
#
# Why this exists: Nightlio keys users by the OIDC `sub` claim (users
# .external_id). Two situations orphan an account's data:
#   - upgrading from the old Google OAuth build: the account is stored as
#     auth_provider='legacy-google' and can no longer log in at all;
#   - the Pocket ID instance was recreated: same human, new sub UUID, so
#     their next login creates a fresh empty account.
# In both cases the fix is the same: point the OLD row (which owns the data)
# at the NEW sub, and delete the empty auto-created row.
#
# Usage:
#   ./relink-oidc-user.sh --list
#       Show all users with id, provider, external_id, email and row counts.
#   ./relink-oidc-user.sh <keep_user_id> <new_oidc_sub>
#       Rewrites user <keep_user_id> to auth_provider='oidc',
#       external_id=<new_oidc_sub>. If another user row already holds that
#       sub (the empty auto-created one), it is deleted first — but only if
#       it owns no mood entries and no goals; otherwise the script aborts.
#
# Runs against the api container (default name nightlio-api, override with
# NIGHTLIO_API_CONTAINER) using its own python + DATABASE_PATH, so it works
# wherever the container runs and never needs sqlite3 on the host.

set -euo pipefail

CONTAINER="${NIGHTLIO_API_CONTAINER:-nightlio-api}"

run_py() {
  docker exec -i "$CONTAINER" python - "$@"
}

if [ "${1:-}" = "--list" ]; then
  run_py <<'PY'
import os, sqlite3
db = sqlite3.connect(os.environ.get("DATABASE_PATH", "/app/data/nightlio.db"))
db.row_factory = sqlite3.Row
rows = db.execute("""
    SELECT u.id, u.auth_provider, u.external_id, u.email, u.name,
           (SELECT COUNT(*) FROM mood_entries m WHERE m.user_id = u.id) entries,
           (SELECT COUNT(*) FROM goals g WHERE g.user_id = u.id) goals
      FROM users u ORDER BY u.id
""").fetchall()
print(f"{'id':>5}  {'provider':<14} {'entries':>7} {'goals':>5}  {'email':<32} external_id")
for r in rows:
    print(f"{r['id']:>5}  {r['auth_provider'] or '?':<14} {r['entries']:>7} {r['goals']:>5}  {r['email']:<32} {r['external_id']}")
PY
  exit 0
fi

KEEP_ID="${1:?usage: relink-oidc-user.sh <keep_user_id> <new_oidc_sub>  (or --list)}"
NEW_SUB="${2:?usage: relink-oidc-user.sh <keep_user_id> <new_oidc_sub>  (or --list)}"

KEEP_ID="$KEEP_ID" NEW_SUB="$NEW_SUB" docker exec -i \
  -e KEEP_ID -e NEW_SUB "$CONTAINER" python - <<'PY'
import os, sqlite3, sys

keep_id = int(os.environ["KEEP_ID"])
new_sub = os.environ["NEW_SUB"]

db = sqlite3.connect(os.environ.get("DATABASE_PATH", "/app/data/nightlio.db"))
db.row_factory = sqlite3.Row
db.execute("BEGIN IMMEDIATE")
try:
    keep = db.execute("SELECT * FROM users WHERE id = ?", (keep_id,)).fetchone()
    if keep is None:
        sys.exit(f"no user with id {keep_id}")

    dupe = db.execute(
        "SELECT * FROM users WHERE auth_provider = 'oidc' AND external_id = ? AND id != ?",
        (new_sub, keep_id),
    ).fetchone()
    if dupe:
        counts = db.execute(
            """SELECT (SELECT COUNT(*) FROM mood_entries WHERE user_id = :id)
                    + (SELECT COUNT(*) FROM goals WHERE user_id = :id) AS n""",
            {"id": dupe["id"]},
        ).fetchone()
        if counts["n"]:
            sys.exit(
                f"refusing: user {dupe['id']} already owns sub {new_sub} and has "
                f"data ({counts['n']} entries+goals). Merge that by hand first."
            )
        # Empty auto-created account: remove it and its default rows.
        db.execute(
            "DELETE FROM group_options WHERE group_id IN (SELECT id FROM groups WHERE user_id = ?)",
            (dupe["id"],),
        )
        for table in ("groups", "activity_log", "user_metrics", "achievements",
                      "goal_completions"):
            db.execute(f"DELETE FROM {table} WHERE user_id = ?", (dupe["id"],))
        db.execute("DELETE FROM users WHERE id = ?", (dupe["id"],))
        print(f"deleted empty duplicate user {dupe['id']}")

    db.execute(
        """UPDATE users
              SET auth_provider = 'oidc',
                  external_id = ?,
                  google_id = 'oidc:' || ?
            WHERE id = ?""",
        (new_sub, new_sub, keep_id),
    )

    # A relinked account can end up with zero tag groups: default groups are
    # only seeded on a user's FIRST login, and the per-user groups migration
    # backfilled legacy shared groups to the seeded self-host user, not to
    # this account. An empty groups list renders no categories in the entry
    # editor, so seed the defaults here if the kept account owns none.
    n_groups = db.execute(
        "SELECT COUNT(*) FROM groups WHERE user_id = ?", (keep_id,)
    ).fetchone()[0]
    if n_groups == 0:
        default_groups = {
            "Emotions": ["happy", "excited", "grateful", "relaxed", "content",
                         "tired", "unsure", "bored", "anxious", "angry",
                         "stressed", "sad", "desperate"],
            "Sleep": ["well-rested", "refreshed", "tired", "exhausted",
                      "restless", "insomniac"],
            "Productivity": ["focused", "motivated", "accomplished",
                             "productive", "procrastinating", "distracted",
                             "overwhelmed", "lazy"],
        }
        for gname, options in default_groups.items():
            gid = db.execute(
                "INSERT INTO groups (name, user_id) VALUES (?, ?)",
                (gname, keep_id),
            ).lastrowid
            db.executemany(
                "INSERT INTO group_options (group_id, name) VALUES (?, ?)",
                [(gid, o) for o in options],
            )
        print(f"user {keep_id} had no tag groups; seeded defaults")

    db.commit()
    print(f"user {keep_id} ({keep['email']}) now maps to oidc sub {new_sub}")
except Exception:
    db.rollback()
    raise
PY
