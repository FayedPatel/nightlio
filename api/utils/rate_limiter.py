"""SQLite-backed rate limiter shared across worker processes.

Why this exists
----------------
The previous implementation kept request counts in a plain in-memory dict
(``request_counts = defaultdict(list)``). In production, ``api/docker_start.py``
runs gunicorn with 4 sync workers, each an independent process with its own
memory. A cap configured as "30 requests/minute" was therefore roughly 4x
looser in practice (each worker enforced its own independent 30/minute), and
any count reset to zero whenever a worker process restarted. That made the
limiter meaningfully weaker than its configuration implied, with no signal
to the operator that this was happening.

This module fixes that by storing rate-limit events in a small SQLite
database file, so every worker process (in fact every process on the host,
gunicorn workers included) reads and writes the same counters. SQLite is
used -- rather than Redis -- to preserve this project's single-container,
SQLite-file self-host story; see api/utils/rate_limiter.py's sibling
api/database.py for the same pattern applied to the main app database.

Storage location
-----------------
Controlled by the ``RATE_LIMIT_DB_PATH`` env var. If unset, it defaults to a
``rate_limit.db`` file in the same directory as the main app database
(``DATABASE_PATH`` env var, or api/config.py's own documented default if
that is unset). This module intentionally reads os.environ directly rather
than importing api.config, to avoid import-time coupling with that module.
(Follow-up: once api/config.py exposes a typed setting for this, route it
through get_config() instead of re-deriving the default here.)

Design
------
Table: ``rate_limit_events(key TEXT NOT NULL, ts REAL NOT NULL)`` with an
index on (key, ts). ``key`` identifies both the caller (IP) and the specific
rate-limited endpoint (module-qualified function name), so two endpoints
configured with different limits never share a budget -- a caller hammering
a 30/min endpoint cannot burn down the sibling 10/min endpoint's allowance
the way a single global "count per IP" bucket would.

Algorithm is a sliding window: on each check we delete events for that key
older than ``now - window_seconds`` (opportunistic pruning -- no separate
cleanup job needed), count what is left, and either reject (count at cap)
or insert a new event row and accept. The delete+count+insert happens inside
a single ``BEGIN IMMEDIATE`` transaction so the check-then-act is atomic
across processes sharing the same SQLite file; WAL mode plus a busy_timeout
handle the cross-process lock contention/waiting instead of raising
"database is locked" errors under normal load.

Failure mode
------------
If the SQLite store itself is unavailable or errors (disk full, permission
error, corruption, etc.), we FAIL OPEN: the request is allowed through and a
warning is logged. A rate limiter is a defense-in-depth measure; if its own
storage breaks, that must not be able to take down login/registration for
every user. This is a deliberate tradeoff -- document it if you touch this
file, don't silently change it to fail closed.
"""

import logging
import os
import sqlite3
import time
from functools import wraps
from pathlib import Path

from flask import request, jsonify, current_app

# Support running as a package (api.*) and from within the api/ directory
try:
    from api.utils.is_truthy import is_truthy
except ImportError:  # pragma: no cover - fallback for running from inside api/
    from utils.is_truthy import is_truthy  # type: ignore

logger = logging.getLogger(__name__)

# How long a connection will wait on a lock held by another process/thread
# before giving up, in milliseconds. Generous because rate-limited endpoints
# (login/register) are low-volume and correctness matters more than latency.
_BUSY_TIMEOUT_MS = 5000


def _default_database_path() -> str:
    """Mirror api/config.py's Config.DATABASE_PATH default.

    api/config.py is owned by a concurrent workflow at the time this module
    was written, so we intentionally do not import it (see module docstring).
    This reproduces its documented default: ``<project_root>/data/nightlio.db``.
    """
    project_root = Path(__file__).resolve().parent.parent.parent
    return str(project_root / "data" / "nightlio.db")


def _rate_limit_db_path() -> str:
    """Resolve where the rate-limit SQLite file lives.

    Priority: explicit ``RATE_LIMIT_DB_PATH`` env var, then a
    ``rate_limit.db`` file next to ``DATABASE_PATH`` (env var or the
    project's documented default directory).
    """
    override = os.environ.get("RATE_LIMIT_DB_PATH")
    if override:
        return override

    database_path = os.environ.get("DATABASE_PATH") or _default_database_path()
    directory = os.path.dirname(database_path) or "."
    return os.path.join(directory, "rate_limit.db")


def _get_connection() -> sqlite3.Connection:
    db_path = _rate_limit_db_path()
    directory = os.path.dirname(db_path)
    if directory:
        os.makedirs(directory, exist_ok=True)

    conn = sqlite3.connect(db_path, timeout=_BUSY_TIMEOUT_MS / 1000)
    conn.execute(f"PRAGMA busy_timeout = {_BUSY_TIMEOUT_MS}")
    # WAL allows concurrent readers/writers across processes without the
    # heavier locking of the default rollback journal -- needed since every
    # gunicorn worker opens its own connection to this same file.
    conn.execute("PRAGMA journal_mode = WAL")
    conn.execute(
        "CREATE TABLE IF NOT EXISTS rate_limit_events ("
        "key TEXT NOT NULL, "
        "ts REAL NOT NULL"
        ")"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_rate_limit_events_key_ts "
        "ON rate_limit_events (key, ts)"
    )
    return conn


def _client_ip() -> str:
    """Resolve the caller's IP address for rate-limit bucketing.

    ``X-Forwarded-For`` is trivially spoofable by any direct client -- it is
    only meaningful if a trusted reverse proxy in front of this app sets (and
    strips any client-supplied value for) that header. We therefore only
    honor it when the operator has explicitly opted in via
    ``TRUST_PROXY_HEADERS`` (truthy per api/utils/is_truthy.py). By default
    this is off, and we fall back to Flask's ``remote_addr``, which is the
    actual TCP peer address and cannot be spoofed by the client.

    Self-hoster note: if Nightlio sits behind nginx/Traefik/Cloudflare etc.
    that sets X-Forwarded-For itself and strips any client-supplied copy,
    set ``TRUST_PROXY_HEADERS=1`` (or ``true``/``yes``/``on``) so rate limits
    are applied per real client instead of per proxy IP. Leave it unset/off
    if the app is reachable directly, or if the proxy does not strip the
    header, or you will allow rate-limit bypass via a forged header.
    """
    if is_truthy(os.environ.get("TRUST_PROXY_HEADERS")):
        forwarded = request.environ.get("HTTP_X_FORWARDED_FOR")
        if forwarded:
            return forwarded.split(",")[0].strip()
    return request.remote_addr or "unknown"


def _check_and_record(key: str, max_requests: int, window_seconds: float) -> bool:
    """Atomically check-and-record one request against the sliding window.

    Returns True if the request is allowed (and has been recorded), False if
    the caller is at/over the cap for this window (nothing is recorded).
    Raises sqlite3.Error on storage failure; callers decide the failure mode.
    """
    now = time.time()
    window_start = now - window_seconds

    conn = _get_connection()
    try:
        conn.execute("BEGIN IMMEDIATE")
        # Opportunistic prune: drop this key's events that have aged out of
        # the window. Cheap (indexed) and means no separate cleanup job is
        # needed to keep the table small.
        conn.execute(
            "DELETE FROM rate_limit_events WHERE key = ? AND ts < ?",
            (key, window_start),
        )
        row = conn.execute(
            "SELECT COUNT(*) FROM rate_limit_events WHERE key = ? AND ts >= ?",
            (key, window_start),
        ).fetchone()
        count = row[0] if row else 0

        if count >= max_requests:
            conn.execute("COMMIT")
            return False

        conn.execute(
            "INSERT INTO rate_limit_events (key, ts) VALUES (?, ?)", (key, now)
        )
        conn.execute("COMMIT")
        return True
    except sqlite3.Error:
        try:
            conn.execute("ROLLBACK")
        except sqlite3.Error:
            pass
        raise
    finally:
        conn.close()


def rate_limit(max_requests=100, window_minutes=15):
    """Rate limiting decorator.

    Public signature is unchanged from the previous in-memory implementation,
    so existing call sites (api/routes/auth_routes.py) require no changes.
    """

    def decorator(f):
        # Module-qualified function identity, fixed once at decoration time.
        # Included in the storage key so different rate-limited endpoints
        # (e.g. /auth/verify at 30/min vs /auth/login at 10/min) never share
        # a budget with each other.
        bucket_prefix = f"{f.__module__}.{f.__qualname__}"

        @wraps(f)
        def decorated_function(*args, **kwargs):
            if current_app.config.get("TESTING"):
                return f(*args, **kwargs)

            client_ip = _client_ip()
            key = f"{bucket_prefix}:{client_ip}"
            window_seconds = window_minutes * 60

            try:
                allowed = _check_and_record(key, max_requests, window_seconds)
            except sqlite3.Error:
                # Fail OPEN: see module docstring. A broken rate-limit store
                # must not be able to take down login/registration for every
                # user. Logged as a warning so operators notice and fix the
                # underlying storage problem instead of silently losing this
                # protection.
                logger.warning(
                    "Rate limiter storage error; allowing request through "
                    "(fail-open)",
                    exc_info=True,
                )
                allowed = True

            if not allowed:
                return (
                    jsonify(
                        {"error": "Rate limit exceeded. Please try again later."}
                    ),
                    429,
                )

            return f(*args, **kwargs)

        return decorated_function

    return decorator
