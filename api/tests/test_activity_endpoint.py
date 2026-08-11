import uuid

from api.app import create_app


def _auth(token):
    return {"Authorization": f"Bearer {token}"}


def _bootstrap_user(app, client):
    """Register a fresh local user, log in, and return (user_id, token)."""
    resp = client.post("/api/auth/local/login")
    assert resp.status_code == 200
    admin_token = resp.get_json()["token"]

    username = f"user-{uuid.uuid4().hex[:12]}"
    resp = client.post(
        "/api/auth/local/register",
        json={"username": username, "password": "long-enough-pw"},
        headers=_auth(admin_token),
    )
    assert resp.status_code == 201
    user_id = resp.get_json()["user"]["id"]

    login = client.post(
        "/api/auth/local/login",
        json={"username": username, "password": "long-enough-pw"},
    )
    assert login.status_code == 200
    return user_id, login.get_json()["token"]


def test_activity_requires_auth():
    app = create_app("testing")
    client = app.test_client()
    assert client.get("/api/activity").status_code == 401


def test_activity_pagination_and_isolation():
    app = create_app("testing")
    client = app.test_client()
    db = app.extensions["user_service"].db

    user_id, token = _bootstrap_user(app, client)
    other_id, other_token = _bootstrap_user(app, client)

    # Seed a deterministic set of events for the first user (the login above
    # already recorded one "login" event for each user).
    seeded_ids = [
        db.add_activity(user_id, "entry_created", {"entry_id": n}) for n in range(6)
    ]
    db.add_activity(other_id, "entry_created", {"entry_id": 999})

    # First page: newest first, full page yields a cursor.
    resp = client.get("/api/activity?limit=3", headers=_auth(token))
    assert resp.status_code == 200
    page1 = resp.get_json()
    assert len(page1["activities"]) == 3
    ids1 = [a["id"] for a in page1["activities"]]
    assert ids1 == sorted(ids1, reverse=True)
    assert ids1[0] == seeded_ids[-1]
    assert page1["next_cursor"] == ids1[-1]

    # Second page continues strictly below the cursor with no overlap.
    resp = client.get(
        f"/api/activity?limit=3&before={page1['next_cursor']}", headers=_auth(token)
    )
    assert resp.status_code == 200
    page2 = resp.get_json()
    ids2 = [a["id"] for a in page2["activities"]]
    assert len(ids2) == 3
    assert max(ids2) < min(ids1)
    assert not set(ids1) & set(ids2)

    # Every event belongs to the requesting user; the other user's event
    # never leaks in.
    resp = client.get("/api/activity?limit=200", headers=_auth(token))
    all_mine = resp.get_json()["activities"]
    assert all(a["user_id"] == user_id for a in all_mine)

    # A short page terminates pagination.
    assert resp.get_json()["next_cursor"] is None

    # The other user sees only their own feed.
    resp = client.get("/api/activity?limit=200", headers=_auth(other_token))
    other_feed = resp.get_json()["activities"]
    assert all(a["user_id"] == other_id for a in other_feed)
    assert any(a["event_type"] == "login" for a in other_feed)


def test_activity_rejects_bad_params():
    app = create_app("testing")
    client = app.test_client()
    _user_id, token = _bootstrap_user(app, client)

    assert (
        client.get("/api/activity?before=abc", headers=_auth(token)).status_code == 400
    )
    assert (
        client.get("/api/activity?limit=abc", headers=_auth(token)).status_code == 400
    )
