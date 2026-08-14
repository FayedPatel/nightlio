"""Direct coverage for POST /api/mood (previously only exercised as setup
in the update-endpoint tests), including the backdated-entry contract."""

from datetime import date, timedelta


def _payload(**overrides):
    body = {
        "mood": 4,
        "date": date.today().isoformat(),
        "content": "A pretty good day.",
        "selected_options": [],
    }
    body.update(overrides)
    return body


def test_create_entry_returns_201_and_id(client, auth_headers):
    resp = client.post("/api/mood", json=_payload(), headers=auth_headers)
    assert resp.status_code == 201, resp.get_data(as_text=True)
    data = resp.get_json()
    assert data["status"] == "success"
    assert isinstance(data["entry_id"], int)


def test_create_backdated_entry_persists_date(client, auth_headers):
    yesterday = (date.today() - timedelta(days=1)).isoformat()
    resp = client.post(
        "/api/mood", json=_payload(date=yesterday), headers=auth_headers
    )
    assert resp.status_code == 201
    entry_id = resp.get_json()["entry_id"]

    entries = client.get("/api/moods", headers=auth_headers).get_json()
    match = next(entry for entry in entries if entry["id"] == entry_id)
    assert match["date"] == yesterday


def test_create_entry_accepts_legacy_us_date_format(client, auth_headers):
    today = date.today()
    legacy = f"{today.month}/{today.day}/{today.year}"
    resp = client.post("/api/mood", json=_payload(date=legacy), headers=auth_headers)
    assert resp.status_code == 201


def test_create_entry_rejects_future_date(client, auth_headers):
    future = (date.today() + timedelta(days=7)).isoformat()
    resp = client.post("/api/mood", json=_payload(date=future), headers=auth_headers)
    assert resp.status_code == 400
    assert "future" in resp.get_json()["error"]


def test_create_entry_rejects_malformed_date(client, auth_headers):
    resp = client.post(
        "/api/mood", json=_payload(date="not-a-date"), headers=auth_headers
    )
    assert resp.status_code == 400


def test_create_entry_rejects_missing_fields(client, auth_headers):
    resp = client.post(
        "/api/mood", json={"mood": 3, "date": date.today().isoformat()},
        headers=auth_headers,
    )
    assert resp.status_code == 400


def test_backdated_entry_sorts_under_its_day(client, auth_headers):
    # Create today's entry first, then backdate one: history order must be
    # by journaled day, not creation time.
    today = date.today().isoformat()
    yesterday = (date.today() - timedelta(days=1)).isoformat()
    client.post("/api/mood", json=_payload(date=today), headers=auth_headers)
    client.post(
        "/api/mood",
        json=_payload(date=yesterday, content="Backdated."),
        headers=auth_headers,
    )

    entries = client.get("/api/moods", headers=auth_headers).get_json()
    dates = [entry["date"] for entry in entries]
    assert dates.index(today) < dates.index(yesterday)


def test_streak_counts_backdated_day_and_dedupes_formats(client, auth_headers):
    # Same day stored in both shapes must count once, and a backdated
    # yesterday plus today makes a 2-day streak.
    today = date.today()
    yesterday = today - timedelta(days=1)
    client.post(
        "/api/mood", json=_payload(date=today.isoformat()), headers=auth_headers
    )
    client.post(
        "/api/mood",
        json=_payload(date=f"{today.month}/{today.day}/{today.year}"),
        headers=auth_headers,
    )
    client.post(
        "/api/mood", json=_payload(date=yesterday.isoformat()), headers=auth_headers
    )

    resp = client.get("/api/streak", headers=auth_headers)
    assert resp.status_code == 200, resp.get_data(as_text=True)
    assert resp.get_json()["current_streak"] == 2
