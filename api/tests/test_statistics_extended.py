"""Tests for the Phase 3 extended statistics aggregations and endpoints.

Mixin-level tests seed a throwaway database and assert exact numbers for each
aggregation, including empty-data and cross-user isolation cases. Endpoint
tests exercise auth gating, parameter validation, and the empty-database
response shape (200 with empty/null structures, never 500).
"""

import os
import sqlite3
import time
import uuid
from datetime import date, datetime, timedelta, timezone

import pytest

from api.app import create_app
from api.database import MoodDatabase


# ---------------------------------------------------------------------------
# Mixin-level fixtures and helpers
# ---------------------------------------------------------------------------


@pytest.fixture()
def db(tmp_path):
    return MoodDatabase(str(tmp_path / "stats_test.db"))


@pytest.fixture()
def alice(db):
    return db.create_local_user("alice", "hash-a")


@pytest.fixture()
def bob(db):
    return db.create_local_user("bob", "hash-b")


def _add_entry(db, user_id, day, mood, options=None):
    return db.add_mood_entry(user_id, day, mood, f"entry {day}", None, options or [])


def _add_goal_completion(db, user_id, goal_id, day):
    with sqlite3.connect(db.db_path) as conn:
        conn.execute(
            "INSERT INTO goal_completions (user_id, goal_id, date) VALUES (?, ?, ?)",
            (user_id, goal_id, day),
        )
        conn.commit()


# ---------------------------------------------------------------------------
# Rolling averages
# ---------------------------------------------------------------------------


def test_rolling_averages_exact(db, alice):
    moods = [1, 2, 3, 4, 5, 1, 2, 3, 4, 5]
    for offset, mood in enumerate(moods):
        _add_entry(db, alice, f"2026-01-{offset + 1:02d}", mood)

    result = db.rolling_averages(alice)
    series = result["series"]
    assert result["count"] == 10
    assert len(series) == 10

    first = series[0]
    assert first["date"] == "2026-01-01"
    assert first["average_mood"] == pytest.approx(1.0)
    assert first["entry_count"] == 1
    assert first["rolling_7"] == pytest.approx(1.0)
    assert first["rolling_30"] == pytest.approx(1.0)

    seventh = series[6]
    assert seventh["date"] == "2026-01-07"
    assert seventh["rolling_7"] == pytest.approx(sum(moods[:7]) / 7)

    last = series[9]
    assert last["date"] == "2026-01-10"
    assert last["rolling_7"] == pytest.approx(sum(moods[3:10]) / 7)
    assert last["rolling_30"] == pytest.approx(sum(moods) / 10)


def test_rolling_averages_groups_multiple_entries_per_day(db, alice):
    _add_entry(db, alice, "2026-01-01", 2)
    _add_entry(db, alice, "2026-01-01", 4)

    result = db.rolling_averages(alice)
    assert result["count"] == 1
    point = result["series"][0]
    assert point["average_mood"] == pytest.approx(3.0)
    assert point["entry_count"] == 2
    assert point["rolling_7"] == pytest.approx(3.0)


def test_rolling_averages_normalises_us_locale_dates(db, alice):
    # The frontend's toLocaleDateString() produces M/D/YYYY; older rows may
    # already be ISO. Both must land in one ordered ISO series.
    _add_entry(db, alice, "1/2/2026", 4)
    _add_entry(db, alice, "2026-01-03", 2)

    result = db.rolling_averages(alice)
    assert [p["date"] for p in result["series"]] == ["2026-01-02", "2026-01-03"]


def test_rolling_averages_empty(db, alice):
    assert db.rolling_averages(alice) == {"series": [], "count": 0}


def test_rolling_averages_cross_user_isolation(db, alice, bob):
    _add_entry(db, bob, "2026-01-01", 5)
    assert db.rolling_averages(alice) == {"series": [], "count": 0}
    assert db.rolling_averages(bob)["count"] == 1


# ---------------------------------------------------------------------------
# Weekday averages
# ---------------------------------------------------------------------------


def test_weekday_averages_exact(db, alice):
    # 2026-01-04 and 2026-01-11 are Sundays; 2026-01-05 is a Monday.
    _add_entry(db, alice, "2026-01-04", 5)
    _add_entry(db, alice, "2026-01-11", 3)
    _add_entry(db, alice, "2026-01-05", 2)

    result = db.weekday_averages(alice)
    assert len(result) == 7

    sunday = result[0]
    assert sunday["weekday"] == 0
    assert sunday["name"] == "Sunday"
    assert sunday["average_mood"] == pytest.approx(4.0)
    assert sunday["entry_count"] == 2

    monday = result[1]
    assert monday["average_mood"] == pytest.approx(2.0)
    assert monday["entry_count"] == 1

    for row in result[2:]:
        assert row["average_mood"] is None
        assert row["entry_count"] == 0


def test_weekday_averages_empty(db, alice):
    result = db.weekday_averages(alice)
    assert len(result) == 7
    assert all(r["entry_count"] == 0 and r["average_mood"] is None for r in result)


def test_weekday_averages_cross_user_isolation(db, alice, bob):
    _add_entry(db, bob, "2026-01-04", 5)
    assert all(r["entry_count"] == 0 for r in db.weekday_averages(alice))


# ---------------------------------------------------------------------------
# Mood volatility
# ---------------------------------------------------------------------------


def test_mood_volatility_exact(db, alice):
    today = date.today()
    _add_entry(db, alice, today.isoformat(), 1)
    _add_entry(db, alice, (today - timedelta(days=1)).isoformat(), 3)
    _add_entry(db, alice, (today - timedelta(days=2)).isoformat(), 5)
    # Far outside the 30-day window; must be excluded.
    _add_entry(db, alice, (today - timedelta(days=100)).isoformat(), 5)

    result = db.mood_volatility(alice)
    assert result["window_days"] == 30
    assert result["entry_count"] == 3
    assert result["average_mood"] == pytest.approx(3.0)
    # Sample stddev of [1, 3, 5] is exactly 2.
    assert result["stddev"] == pytest.approx(2.0)


def test_mood_volatility_single_entry_has_null_stddev(db, alice):
    _add_entry(db, alice, date.today().isoformat(), 4)
    result = db.mood_volatility(alice)
    assert result["entry_count"] == 1
    assert result["average_mood"] == pytest.approx(4.0)
    assert result["stddev"] is None


def test_mood_volatility_identical_moods_zero_stddev(db, alice):
    today = date.today()
    _add_entry(db, alice, today.isoformat(), 3)
    _add_entry(db, alice, (today - timedelta(days=1)).isoformat(), 3)
    result = db.mood_volatility(alice)
    assert result["stddev"] == pytest.approx(0.0)


def test_mood_volatility_empty(db, alice):
    result = db.mood_volatility(alice)
    assert result == {
        "window_days": 30,
        "entry_count": 0,
        "average_mood": None,
        "stddev": None,
    }


def test_mood_volatility_rejects_bad_window(db, alice):
    with pytest.raises(ValueError):
        db.mood_volatility(alice, days=0)


def test_mood_volatility_cross_user_isolation(db, alice, bob):
    _add_entry(db, bob, date.today().isoformat(), 5)
    assert db.mood_volatility(alice)["entry_count"] == 0


def test_mood_volatility_window_uses_local_time(db, alice):
    # Entry dates are written from the client's local calendar, so the
    # trailing window must be anchored to the local date, not UTC. Pick a
    # fixed-offset timezone in which the local date currently differs from
    # the UTC date; a UTC-anchored window then misplaces the boundary by a
    # full day in whichever direction applies.
    utc_now = datetime.now(timezone.utc)
    if utc_now.hour < 12:
        # UTC-12: the local date is still yesterday relative to UTC, so a
        # UTC-anchored cutoff would wrongly exclude the oldest in-window day.
        tz_name, offset = "TST12", timedelta(hours=-12)
    else:
        # UTC+14: the local date is already tomorrow relative to UTC, so a
        # UTC-anchored cutoff would wrongly include a day past the window.
        tz_name, offset = "TST-14", timedelta(hours=14)

    old_tz = os.environ.get("TZ")
    os.environ["TZ"] = tz_name
    time.tzset()
    try:
        local_today = (utc_now + offset).date()
        # Oldest day inside the 30-day window: must be counted.
        _add_entry(db, alice, (local_today - timedelta(days=29)).isoformat(), 4)
        # One day past the window: must be excluded.
        _add_entry(db, alice, (local_today - timedelta(days=30)).isoformat(), 1)

        result = db.mood_volatility(alice)
        assert result["entry_count"] == 1
        assert result["average_mood"] == pytest.approx(4.0)
    finally:
        if old_tz is None:
            os.environ.pop("TZ", None)
        else:
            os.environ["TZ"] = old_tz
        time.tzset()


# ---------------------------------------------------------------------------
# Tag correlations
# ---------------------------------------------------------------------------


def _seed_alice_tag_data(db, alice):
    group_id = db.create_group("Habits", user_id=alice)
    option_id = db.create_group_option(group_id, "Exercise", user_id=alice)
    # Selected days: 2026-02-01 (entries 5 and 3 — the 3 has no explicit
    # selection but shares the day), 2026-02-02 (entry 4).
    _add_entry(db, alice, "2026-02-01", 5, [option_id])
    _add_entry(db, alice, "2026-02-01", 3)
    _add_entry(db, alice, "2026-02-02", 4, [option_id])
    # Unselected days.
    _add_entry(db, alice, "2026-02-03", 2)
    _add_entry(db, alice, "2026-02-04", 1)
    return group_id, option_id


def _assert_alice_tag_numbers(row):
    assert row["option_name"] == "Exercise"
    assert row["group_name"] == "Habits"
    assert row["average_mood_selected"] == pytest.approx(4.0)
    assert row["entry_count_selected"] == 3
    assert row["average_mood_not_selected"] == pytest.approx(1.5)
    assert row["entry_count_not_selected"] == 2


def test_tag_correlations_exact(db, alice):
    _seed_alice_tag_data(db, alice)
    result = db.tag_correlations(alice)
    assert len(result) == 1
    _assert_alice_tag_numbers(result[0])


def test_tag_correlations_empty(db, alice):
    assert db.tag_correlations(alice) == []


def test_tag_correlations_cross_user_isolation(db, alice, bob):
    _, alice_option = _seed_alice_tag_data(db, alice)

    # Bob logs entries selecting Alice's option id directly at the DB layer.
    # His data must never move Alice's numbers.
    _add_entry(db, bob, "2026-02-01", 1, [alice_option])
    _add_entry(db, bob, "2026-02-03", 1, [alice_option])

    result = db.tag_correlations(alice)
    assert len(result) == 1
    _assert_alice_tag_numbers(result[0])

    # Bob's selections point at a group he does not own. Surfacing it would
    # leak Alice's group and option names to Bob, so he gets no rows at all.
    assert db.tag_correlations(bob) == []


def test_tag_correlations_never_leak_other_users_group_names(db, alice, bob):
    # Regression test: selecting another user's option id (option ids are
    # sequential and guessable) must not expose the owner's group_name or
    # group_id through the correlations of the selecting user.
    _, alice_option = _seed_alice_tag_data(db, alice)
    _add_entry(db, bob, "2026-02-01", 1, [alice_option])

    for row in db.tag_correlations(bob):
        assert row["group_name"] != "Habits"
        assert row["option_name"] != "Exercise"


# ---------------------------------------------------------------------------
# Goal correlations
# ---------------------------------------------------------------------------


def test_goal_correlations_exact(db, alice):
    goal_id = db.create_goal(alice, "Meditate", "", 7)
    _add_goal_completion(db, alice, goal_id, "2026-03-01")
    _add_goal_completion(db, alice, goal_id, "2026-03-03")

    _add_entry(db, alice, "2026-03-01", 5)
    _add_entry(db, alice, "2026-03-02", 1)
    _add_entry(db, alice, "2026-03-03", 3)
    _add_entry(db, alice, "2026-03-04", 3)

    result = db.goal_correlations(alice)
    assert len(result) == 1
    row = result[0]
    assert row["goal_id"] == goal_id
    assert row["goal_name"] == "Meditate"
    assert row["average_mood_completed"] == pytest.approx(4.0)
    assert row["entry_count_completed"] == 2
    assert row["average_mood_not_completed"] == pytest.approx(2.0)
    assert row["entry_count_not_completed"] == 2


def test_goal_correlations_goal_without_completions(db, alice):
    db.create_goal(alice, "Read", "", 3)
    _add_entry(db, alice, "2026-03-01", 4)

    result = db.goal_correlations(alice)
    assert len(result) == 1
    row = result[0]
    assert row["average_mood_completed"] is None
    assert row["entry_count_completed"] == 0
    assert row["average_mood_not_completed"] == pytest.approx(4.0)
    assert row["entry_count_not_completed"] == 1


def test_goal_correlations_empty(db, alice):
    assert db.goal_correlations(alice) == []
    # A goal with no mood entries at all still yields no correlation rows.
    db.create_goal(alice, "Sleep", "", 7)
    assert db.goal_correlations(alice) == []


def test_goal_correlations_cross_user_isolation(db, alice, bob):
    alice_goal = db.create_goal(alice, "Meditate", "", 7)
    _add_goal_completion(db, alice, alice_goal, "2026-03-01")
    _add_entry(db, alice, "2026-03-01", 5)
    _add_entry(db, alice, "2026-03-02", 1)

    bob_goal = db.create_goal(bob, "Meditate", "", 7)
    _add_goal_completion(db, bob, bob_goal, "2026-03-02")
    _add_entry(db, bob, "2026-03-02", 5)

    result = db.goal_correlations(alice)
    assert len(result) == 1
    row = result[0]
    assert row["goal_id"] == alice_goal
    assert row["average_mood_completed"] == pytest.approx(5.0)
    assert row["entry_count_completed"] == 1
    assert row["average_mood_not_completed"] == pytest.approx(1.0)
    assert row["entry_count_not_completed"] == 1


# ---------------------------------------------------------------------------
# Heatmap
# ---------------------------------------------------------------------------


def test_heatmap_exact(db, alice):
    _add_entry(db, alice, "2026-05-01", 3)
    _add_entry(db, alice, "2026-05-01", 5)
    _add_entry(db, alice, "2026-07-15", 2)
    # Different year; must be excluded.
    _add_entry(db, alice, "2025-12-31", 1)

    result = db.heatmap(alice, 2026)
    assert result["year"] == 2026
    assert result["days_logged"] == 2
    assert result["days"][0] == {
        "date": "2026-05-01",
        "average_mood": pytest.approx(4.0),
        "entry_count": 2,
    }
    assert result["days"][1] == {
        "date": "2026-07-15",
        "average_mood": pytest.approx(2.0),
        "entry_count": 1,
    }


def test_heatmap_empty(db, alice):
    assert db.heatmap(alice, 2026) == {"year": 2026, "days": [], "days_logged": 0}


def test_heatmap_cross_user_isolation(db, alice, bob):
    _add_entry(db, bob, "2026-05-01", 3)
    assert db.heatmap(alice, 2026)["days_logged"] == 0


# ---------------------------------------------------------------------------
# Monthly digest
# ---------------------------------------------------------------------------


def test_monthly_digest_exact(db, alice):
    group_id = db.create_group("Habits", user_id=alice)
    option_a = db.create_group_option(group_id, "Exercise", user_id=alice)
    option_b = db.create_group_option(group_id, "Reading", user_id=alice)

    _add_entry(db, alice, "2026-03-01", 4, [option_a])
    _add_entry(db, alice, "2026-03-02", 2, [option_a, option_b])
    _add_entry(db, alice, "2026-03-05", 3)
    # Previous month.
    _add_entry(db, alice, "2026-02-10", 1)

    digest = db.monthly_digest(alice, 2026, 3)
    assert digest["year"] == 2026
    assert digest["month"] == 3
    assert digest["entries_logged"] == 3
    assert digest["average_mood"] == pytest.approx(3.0)
    assert digest["previous_average_mood"] == pytest.approx(1.0)
    assert digest["mood_trend"] == pytest.approx(2.0)
    assert digest["longest_streak"] == 2

    assert len(digest["top_tags"]) == 2
    assert digest["top_tags"][0]["option_name"] == "Exercise"
    assert digest["top_tags"][0]["times_selected"] == 2
    assert digest["top_tags"][1]["option_name"] == "Reading"
    assert digest["top_tags"][1]["times_selected"] == 1


def test_monthly_digest_january_uses_december_previous_year(db, alice):
    _add_entry(db, alice, "2026-01-05", 4)
    _add_entry(db, alice, "2025-12-20", 2)

    digest = db.monthly_digest(alice, 2026, 1)
    assert digest["average_mood"] == pytest.approx(4.0)
    assert digest["previous_average_mood"] == pytest.approx(2.0)
    assert digest["mood_trend"] == pytest.approx(2.0)


def test_monthly_digest_trend_null_without_previous_month(db, alice):
    _add_entry(db, alice, "2026-03-01", 4)
    digest = db.monthly_digest(alice, 2026, 3)
    assert digest["previous_average_mood"] is None
    assert digest["mood_trend"] is None


def test_monthly_digest_streak_stays_within_month(db, alice):
    _add_entry(db, alice, "2026-04-28", 3)
    _add_entry(db, alice, "2026-04-29", 3)
    _add_entry(db, alice, "2026-04-30", 3)
    _add_entry(db, alice, "2026-05-01", 3)

    assert db.monthly_digest(alice, 2026, 4)["longest_streak"] == 3
    assert db.monthly_digest(alice, 2026, 5)["longest_streak"] == 1


def test_monthly_digest_empty(db, alice):
    digest = db.monthly_digest(alice, 2026, 6)
    assert digest["entries_logged"] == 0
    assert digest["average_mood"] is None
    assert digest["previous_average_mood"] is None
    assert digest["mood_trend"] is None
    assert digest["top_tags"] == []
    assert digest["longest_streak"] == 0


def test_monthly_digest_rejects_bad_month(db, alice):
    with pytest.raises(ValueError):
        db.monthly_digest(alice, 2026, 13)


def test_monthly_digest_cross_user_isolation(db, alice, bob):
    _add_entry(db, bob, "2026-03-01", 5)
    digest = db.monthly_digest(alice, 2026, 3)
    assert digest["entries_logged"] == 0
    assert digest["average_mood"] is None


def test_monthly_digest_top_tags_never_leak_other_users_group_names(db, alice, bob):
    # Regression test: Bob selecting Alice's option id (sequential, so
    # guessable) must not surface Alice's group_name in Bob's digest.
    group_id = db.create_group("Habits", user_id=alice)
    option_id = db.create_group_option(group_id, "Exercise", user_id=alice)

    _add_entry(db, bob, "2026-03-01", 2, [option_id])

    digest = db.monthly_digest(bob, 2026, 3)
    assert digest["entries_logged"] == 1
    assert digest["top_tags"] == []

    # Alice's own digest still counts only her own selections.
    _add_entry(db, alice, "2026-03-02", 4, [option_id])
    alice_digest = db.monthly_digest(alice, 2026, 3)
    assert len(alice_digest["top_tags"]) == 1
    assert alice_digest["top_tags"][0]["option_name"] == "Exercise"
    assert alice_digest["top_tags"][0]["group_name"] == "Habits"
    assert alice_digest["top_tags"][0]["times_selected"] == 1


# ---------------------------------------------------------------------------
# Endpoint tests
# ---------------------------------------------------------------------------


def _make_client():
    app = create_app("testing")
    return app.test_client()


def _auth(token):
    return {"Authorization": f"Bearer {token}"}


def _register_and_login(client, admin_token):
    username = f"user-{uuid.uuid4().hex[:12]}"
    resp = client.post(
        "/api/auth/local/register",
        json={"username": username, "password": "long-enough-pw"},
        headers=_auth(admin_token),
    )
    assert resp.status_code == 201
    login = client.post(
        "/api/auth/local/login",
        json={"username": username, "password": "long-enough-pw"},
    )
    assert login.status_code == 200
    return login.get_json()["token"]


def _bootstrap_users(client, count=1):
    resp = client.post("/api/auth/local/login")
    assert resp.status_code == 200
    admin_token = resp.get_json()["token"]
    return [_register_and_login(client, admin_token) for _ in range(count)]


def test_extended_statistics_endpoints_require_auth():
    client = _make_client()
    assert client.get("/api/statistics/extended").status_code == 401
    assert client.get("/api/statistics/heatmap").status_code == 401
    assert client.get("/api/statistics/digest").status_code == 401


def test_extended_statistics_empty_user_returns_empty_structures():
    client = _make_client()
    (token,) = _bootstrap_users(client)

    resp = client.get("/api/statistics/extended", headers=_auth(token))
    assert resp.status_code == 200
    data = resp.get_json()

    assert data["rolling_averages"] == {"series": [], "count": 0}
    assert len(data["weekday_averages"]) == 7
    assert all(r["entry_count"] == 0 for r in data["weekday_averages"])
    assert data["mood_volatility"]["entry_count"] == 0
    assert data["mood_volatility"]["average_mood"] is None
    assert data["mood_volatility"]["stddev"] is None
    assert data["tag_correlations"] == []
    assert data["goal_correlations"] == []
    assert data["monthly_digest"]["entries_logged"] == 0
    assert data["monthly_digest"]["top_tags"] == []
    assert data["monthly_digest"]["longest_streak"] == 0

    resp = client.get("/api/statistics/heatmap", headers=_auth(token))
    assert resp.status_code == 200
    assert resp.get_json()["days"] == []

    resp = client.get("/api/statistics/digest", headers=_auth(token))
    assert resp.status_code == 200
    assert resp.get_json()["entries_logged"] == 0


def test_heatmap_endpoint_param_validation():
    client = _make_client()
    (token,) = _bootstrap_users(client)

    assert (
        client.get("/api/statistics/heatmap?year=abc", headers=_auth(token)).status_code
        == 400
    )
    assert (
        client.get(
            "/api/statistics/heatmap?year=1800", headers=_auth(token)
        ).status_code
        == 400
    )
    assert (
        client.get(
            "/api/statistics/heatmap?year=2200", headers=_auth(token)
        ).status_code
        == 400
    )

    resp = client.get("/api/statistics/heatmap", headers=_auth(token))
    assert resp.status_code == 200
    assert resp.get_json()["year"] == date.today().year

    resp = client.get("/api/statistics/heatmap?year=2026", headers=_auth(token))
    assert resp.status_code == 200
    assert resp.get_json()["year"] == 2026


def test_digest_endpoint_param_validation():
    client = _make_client()
    (token,) = _bootstrap_users(client)

    for query in ("month=0", "month=13", "month=abc", "year=abc", "year=1800"):
        resp = client.get(f"/api/statistics/digest?{query}", headers=_auth(token))
        assert resp.status_code == 400, query

    resp = client.get("/api/statistics/digest", headers=_auth(token))
    assert resp.status_code == 200
    body = resp.get_json()
    assert body["year"] == date.today().year
    assert body["month"] == date.today().month

    resp = client.get(
        "/api/statistics/digest?year=2026&month=3", headers=_auth(token)
    )
    assert resp.status_code == 200
    assert resp.get_json()["month"] == 3


def test_extended_statistics_reflects_entries_and_keeps_legacy_shape():
    client = _make_client()
    (token,) = _bootstrap_users(client)

    today_iso = date.today().isoformat()
    resp = client.post(
        "/api/mood",
        json={"mood": 4, "date": today_iso, "content": "extended stats test"},
        headers=_auth(token),
    )
    assert resp.status_code == 201

    # The pre-existing /api/statistics response shape must stay unchanged.
    legacy = client.get("/api/statistics", headers=_auth(token))
    assert legacy.status_code == 200
    legacy_body = legacy.get_json()
    assert "statistics" in legacy_body
    assert "mood_distribution" in legacy_body
    assert "current_streak" in legacy_body
    assert legacy_body["statistics"]["total_entries"] == 1

    resp = client.get("/api/statistics/extended", headers=_auth(token))
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["rolling_averages"]["count"] == 1
    point = data["rolling_averages"]["series"][0]
    assert point["date"] == today_iso
    assert point["average_mood"] == pytest.approx(4.0)
    assert data["mood_volatility"]["entry_count"] == 1


def test_extended_statistics_endpoint_cross_user_isolation():
    client = _make_client()
    token_a, token_b = _bootstrap_users(client, count=2)

    resp = client.post(
        "/api/mood",
        json={
            "mood": 5,
            "date": date.today().isoformat(),
            "content": "user A private entry",
        },
        headers=_auth(token_a),
    )
    assert resp.status_code == 201

    data_b = client.get(
        "/api/statistics/extended", headers=_auth(token_b)
    ).get_json()
    assert data_b["rolling_averages"] == {"series": [], "count": 0}
    assert data_b["mood_volatility"]["entry_count"] == 0
