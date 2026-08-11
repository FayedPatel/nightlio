import uuid

from api.app import create_app


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


def _bootstrap_two_users(client):
    resp = client.post("/api/auth/local/login")
    assert resp.status_code == 200
    admin_token = resp.get_json()["token"]
    return _register_and_login(client, admin_token), _register_and_login(
        client, admin_token
    )


def test_group_routes_require_auth():
    client = _make_client()
    assert client.get("/api/groups").status_code == 401
    assert client.post("/api/groups", json={"name": "X"}).status_code == 401
    assert client.delete("/api/groups/1").status_code == 401
    assert client.post("/api/groups/1/options", json={"name": "X"}).status_code == 401
    assert client.delete("/api/options/1").status_code == 401


def test_group_isolation_between_users():
    client = _make_client()
    token_a, token_b = _bootstrap_two_users(client)

    group_name = f"Secret-{uuid.uuid4().hex[:8]}"
    resp = client.post(
        "/api/groups", json={"name": group_name}, headers=_auth(token_a)
    )
    assert resp.status_code == 201
    group_id = resp.get_json()["group_id"]

    # User A sees the group
    groups_a = client.get("/api/groups", headers=_auth(token_a)).get_json()
    assert any(g["id"] == group_id for g in groups_a)

    # User B cannot read it
    groups_b = client.get("/api/groups", headers=_auth(token_b)).get_json()
    assert not any(g["id"] == group_id for g in groups_b)

    # User B cannot add options to it
    resp = client.post(
        f"/api/groups/{group_id}/options",
        json={"name": "sneaky"},
        headers=_auth(token_b),
    )
    assert resp.status_code in (400, 404)

    # User B cannot delete it
    resp = client.delete(f"/api/groups/{group_id}", headers=_auth(token_b))
    assert resp.status_code == 404

    # Still present for user A
    groups_a = client.get("/api/groups", headers=_auth(token_a)).get_json()
    assert any(g["id"] == group_id for g in groups_a)

    # User B cannot delete A's options either
    resp = client.post(
        f"/api/groups/{group_id}/options",
        json={"name": "mine"},
        headers=_auth(token_a),
    )
    assert resp.status_code == 201
    option_id = resp.get_json()["option_id"]

    resp = client.delete(f"/api/options/{option_id}", headers=_auth(token_b))
    assert resp.status_code == 404

    # Owner can delete
    resp = client.delete(f"/api/options/{option_id}", headers=_auth(token_a))
    assert resp.status_code == 200
    resp = client.delete(f"/api/groups/{group_id}", headers=_auth(token_a))
    assert resp.status_code == 200
