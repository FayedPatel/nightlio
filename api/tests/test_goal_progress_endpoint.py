"""POST /api/goals/<id>/progress — including backdated completions.

The endpoint accepts an optional JSON body {date}: the per-day
goal_completions log is idempotent per (goal, day), and the weekly counter
only moves for dates inside the current Monday-based week.
"""

from datetime import datetime, timedelta


def _week_start(d=None):
    d = d or datetime.now().date()
    return d - timedelta(days=d.weekday())


def _iso(d):
    return d.strftime("%Y-%m-%d")


TODAY = datetime.now().date()
TODAY_ISO = _iso(TODAY)
THIS_MONDAY = _week_start()
PREV_MONDAY_ISO = _iso(THIS_MONDAY - timedelta(days=7))


def _create_goal(client, headers, freq=7):
    resp = client.post(
        "/api/goals",
        json={"title": "Test Goal", "description": "desc", "frequency_per_week": freq},
        headers=headers,
    )
    assert resp.status_code in (200, 201)
    return resp.get_json()["id"]


def _completions(client, headers, goal_id):
    resp = client.get(f"/api/goals/{goal_id}/completions", headers=headers)
    assert resp.status_code == 200
    return [row["date"] for row in resp.get_json()]


def test_no_body_logs_today(client, auth_headers):
    goal_id = _create_goal(client, auth_headers)
    resp = client.post(f"/api/goals/{goal_id}/progress", headers=auth_headers)
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["completed"] == 1
    assert data["last_completed_date"] == TODAY_ISO
    assert data["already_logged"] is False
    assert TODAY_ISO in _completions(client, auth_headers, goal_id)


def test_explicit_today_equivalent(client, auth_headers):
    goal_id = _create_goal(client, auth_headers)
    resp = client.post(
        f"/api/goals/{goal_id}/progress",
        json={"date": TODAY_ISO},
        headers=auth_headers,
    )
    data = resp.get_json()
    assert data["completed"] == 1
    assert data["already_completed_today"] is True


def test_same_day_repeat_does_not_double_count(client, auth_headers):
    goal_id = _create_goal(client, auth_headers)
    client.post(f"/api/goals/{goal_id}/progress", headers=auth_headers)
    resp = client.post(f"/api/goals/{goal_id}/progress", headers=auth_headers)
    data = resp.get_json()
    assert data["completed"] == 1
    assert data["already_logged"] is True
    assert _completions(client, auth_headers, goal_id).count(TODAY_ISO) == 1


def test_current_week_backdate_increments(client, auth_headers):
    goal_id = _create_goal(client, auth_headers)
    client.post(f"/api/goals/{goal_id}/progress", headers=auth_headers)

    monday_iso = _iso(THIS_MONDAY)
    resp = client.post(
        f"/api/goals/{goal_id}/progress",
        json={"date": monday_iso},
        headers=auth_headers,
    )
    data = resp.get_json()
    if monday_iso == TODAY_ISO:
        # Today IS Monday: same day as the first call, so no second credit.
        assert data["completed"] == 1
        assert data["already_logged"] is True
    else:
        assert data["completed"] == 2
        assert data["already_logged"] is False
        assert monday_iso in _completions(client, auth_headers, goal_id)
    # Backdating never rolls the today-lock backwards.
    assert data["last_completed_date"] == TODAY_ISO


def test_previous_week_backdate_logs_without_counting(client, auth_headers):
    goal_id = _create_goal(client, auth_headers)
    resp = client.post(
        f"/api/goals/{goal_id}/progress",
        json={"date": PREV_MONDAY_ISO},
        headers=auth_headers,
    )
    data = resp.get_json()
    assert data["completed"] == 0
    assert data["already_logged"] is False
    assert PREV_MONDAY_ISO in _completions(client, auth_headers, goal_id)


def test_backdate_at_weekly_cap_logs_without_exceeding(client, auth_headers):
    goal_id = _create_goal(client, auth_headers, freq=1)
    client.post(f"/api/goals/{goal_id}/progress", headers=auth_headers)
    resp = client.post(
        f"/api/goals/{goal_id}/progress",
        json={"date": PREV_MONDAY_ISO},
        headers=auth_headers,
    )
    data = resp.get_json()
    assert data["completed"] == 1
    completions = _completions(client, auth_headers, goal_id)
    assert TODAY_ISO in completions and PREV_MONDAY_ISO in completions


def test_us_format_normalized_to_iso(client, auth_headers):
    goal_id = _create_goal(client, auth_headers)
    prev_monday = THIS_MONDAY - timedelta(days=7)
    us_format = f"{prev_monday.month}/{prev_monday.day}/{prev_monday.year}"
    resp = client.post(
        f"/api/goals/{goal_id}/progress",
        json={"date": us_format},
        headers=auth_headers,
    )
    assert resp.status_code == 200
    assert PREV_MONDAY_ISO in _completions(client, auth_headers, goal_id)


def test_invalid_date_rejected(client, auth_headers):
    goal_id = _create_goal(client, auth_headers)
    resp = client.post(
        f"/api/goals/{goal_id}/progress",
        json={"date": "not-a-date"},
        headers=auth_headers,
    )
    assert resp.status_code == 400


def test_future_date_rejected(client, auth_headers):
    goal_id = _create_goal(client, auth_headers)
    future_iso = _iso(TODAY + timedelta(days=10))
    resp = client.post(
        f"/api/goals/{goal_id}/progress",
        json={"date": future_iso},
        headers=auth_headers,
    )
    assert resp.status_code == 400
    assert future_iso not in _completions(client, auth_headers, goal_id)
