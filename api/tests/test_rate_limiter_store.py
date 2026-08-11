"""Tests for the SQLite-backed rate limiter (api/utils/rate_limiter.py).

All tests point the store at a tmp_path SQLite file via the
RATE_LIMIT_DB_PATH env var, never at the shared data/nightlio.db used by the
rest of the suite / the running dev server.
"""

import logging
import sqlite3
import time

import pytest
from flask import Flask

from api.utils import rate_limiter as rl


@pytest.fixture(autouse=True)
def _isolated_rate_limit_db(tmp_path, monkeypatch):
    """Point every test at its own throwaway SQLite file."""
    db_path = tmp_path / "rate_limit.db"
    monkeypatch.setenv("RATE_LIMIT_DB_PATH", str(db_path))
    monkeypatch.delenv("TRUST_PROXY_HEADERS", raising=False)
    monkeypatch.delenv("DATABASE_PATH", raising=False)
    return str(db_path)


def _make_app(testing=False):
    app = Flask(__name__)
    app.config["TESTING"] = testing
    return app


# ---------------------------------------------------------------------------
# Core sliding-window behavior (exercised directly against the store).
# ---------------------------------------------------------------------------


def test_under_limit_requests_pass():
    key = "test:under-limit"
    for _ in range(3):
        assert rl._check_and_record(key, max_requests=5, window_seconds=60) is True


def test_over_limit_blocks():
    key = "test:over-limit"
    for _ in range(3):
        assert rl._check_and_record(key, max_requests=3, window_seconds=60) is True
    # 4th request within the same window, same cap: blocked.
    assert rl._check_and_record(key, max_requests=3, window_seconds=60) is False


def test_window_expiry_unblocks():
    key = "test:window-expiry"
    for _ in range(2):
        assert (
            rl._check_and_record(key, max_requests=2, window_seconds=0.2) is True
        )
    assert rl._check_and_record(key, max_requests=2, window_seconds=0.2) is False

    time.sleep(0.3)  # let the 0.2s window fully elapse

    assert rl._check_and_record(key, max_requests=2, window_seconds=0.2) is True


def test_shared_across_separate_connections_same_db_file():
    """Simulates two gunicorn worker processes sharing one SQLite file.

    _check_and_record opens and closes a fresh connection on every call (no
    Python-level shared state), so simply calling it repeatedly against the
    same RATE_LIMIT_DB_PATH already exercises the cross-process sharing that
    the old in-memory dict could not provide -- two workers hitting this
    would see each other's counts, not a private 0-based count each.
    """
    key = "test:multi-worker"
    max_requests = 4

    # "worker A": three requests recorded.
    for _ in range(3):
        assert (
            rl._check_and_record(key, max_requests, window_seconds=60) is True
        )

    # "worker B": sees worker A's 3 prior events via the shared file, so
    # only one more request fits under the cap of 4 before blocking.
    assert rl._check_and_record(key, max_requests, window_seconds=60) is True
    assert rl._check_and_record(key, max_requests, window_seconds=60) is False


def test_different_keys_have_independent_budgets():
    # Different bucket keys (e.g. different rate-limited endpoints) must not
    # share a budget with each other.
    assert rl._check_and_record("test:endpoint-a", 1, window_seconds=60) is True
    assert rl._check_and_record("test:endpoint-a", 1, window_seconds=60) is False
    # endpoint-b is unaffected by endpoint-a being at its cap.
    assert rl._check_and_record("test:endpoint-b", 1, window_seconds=60) is True


# ---------------------------------------------------------------------------
# X-Forwarded-For trust gate.
# ---------------------------------------------------------------------------


def test_xff_ignored_by_default():
    app = _make_app()
    with app.test_request_context(
        "/",
        environ_base={
            "REMOTE_ADDR": "10.0.0.1",
            "HTTP_X_FORWARDED_FOR": "1.2.3.4",
        },
    ):
        assert rl._client_ip() == "10.0.0.1"


def test_xff_honored_when_trust_proxy_headers_enabled(monkeypatch):
    monkeypatch.setenv("TRUST_PROXY_HEADERS", "1")
    app = _make_app()
    with app.test_request_context(
        "/",
        environ_base={
            "REMOTE_ADDR": "10.0.0.1",
            "HTTP_X_FORWARDED_FOR": "1.2.3.4, 5.6.7.8",
        },
    ):
        assert rl._client_ip() == "1.2.3.4"


# ---------------------------------------------------------------------------
# Fail-open behavior on storage errors.
# ---------------------------------------------------------------------------


def test_storage_error_fails_open(monkeypatch, caplog):
    app = _make_app(testing=False)

    @app.route("/protected")
    @rl.rate_limit(max_requests=1, window_minutes=1)
    def protected():
        return "ok", 200

    def _boom(*_args, **_kwargs):
        raise sqlite3.OperationalError("simulated storage failure")

    monkeypatch.setattr(rl, "_check_and_record", _boom)

    client = app.test_client()
    with caplog.at_level(logging.WARNING):
        resp = client.get("/protected")

    assert resp.status_code == 200
    assert resp.data == b"ok"
    assert any("fail-open" in record.message for record in caplog.records)


# ---------------------------------------------------------------------------
# End-to-end decorator behavior (signature compatibility with call sites).
# ---------------------------------------------------------------------------


def test_decorator_enforces_configured_limit_end_to_end():
    app = _make_app(testing=False)

    @app.route("/limited")
    @rl.rate_limit(max_requests=2, window_minutes=1)
    def limited():
        return "ok", 200

    client = app.test_client()
    environ_base = {"REMOTE_ADDR": "9.9.9.9"}

    assert client.get("/limited", environ_base=environ_base).status_code == 200
    assert client.get("/limited", environ_base=environ_base).status_code == 200
    resp = client.get("/limited", environ_base=environ_base)
    assert resp.status_code == 429
    assert resp.get_json()["error"] == "Rate limit exceeded. Please try again later."


def test_decorator_bypassed_when_flask_testing_enabled():
    app = _make_app(testing=True)

    @app.route("/limited")
    @rl.rate_limit(max_requests=1, window_minutes=1)
    def limited():
        return "ok", 200

    client = app.test_client()
    environ_base = {"REMOTE_ADDR": "9.9.9.9"}

    # TESTING=True short-circuits the limiter entirely (existing contract
    # relied on by the rest of the test suite's Flask app fixtures).
    for _ in range(5):
        assert client.get("/limited", environ_base=environ_base).status_code == 200
