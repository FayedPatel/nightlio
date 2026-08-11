import uuid

import api.config as config_module
from api.app import create_app


def _make_client():
    app = create_app("testing")
    return app.test_client()


def _default_token(client):
    resp = client.post("/api/auth/local/login")
    assert resp.status_code == 200
    return resp.get_json()["token"]


def _auth(token):
    return {"Authorization": f"Bearer {token}"}


def _unique_username():
    return f"user-{uuid.uuid4().hex[:12]}"


def test_local_login_success_without_credentials_when_oidc_unset():
    client = _make_client()
    resp = client.post("/api/auth/local/login")
    assert resp.status_code == 200
    data = resp.get_json()
    assert "token" in data
    assert "user" in data and "id" in data["user"]


def test_local_login_without_credentials_forbidden_when_oidc_configured(monkeypatch):
    monkeypatch.setenv("OIDC_ISSUER_URL", "https://id.example.com")
    monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)
    try:
        client = _make_client()
        resp = client.post("/api/auth/local/login")
        assert resp.status_code == 403
        assert "token" not in (resp.get_json() or {})
    finally:
        # Force a reload for subsequent tests once monkeypatch restores env.
        monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)


def test_register_requires_auth():
    client = _make_client()
    resp = client.post(
        "/api/auth/local/register",
        json={"username": _unique_username(), "password": "long-enough-pw"},
    )
    assert resp.status_code == 401


def test_register_then_login_and_wrong_password():
    client = _make_client()
    token = _default_token(client)

    username = _unique_username()
    resp = client.post(
        "/api/auth/local/register",
        json={"username": username, "password": "correct horse battery"},
        headers=_auth(token),
    )
    assert resp.status_code == 201
    created = resp.get_json()["user"]
    assert created["id"]

    # Correct password logs in
    resp = client.post(
        "/api/auth/local/login",
        json={"username": username, "password": "correct horse battery"},
    )
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["user"]["id"] == created["id"]
    assert "token" in data

    # Wrong password is rejected
    resp = client.post(
        "/api/auth/local/login",
        json={"username": username, "password": "wrong password"},
    )
    assert resp.status_code == 401
    assert "token" not in (resp.get_json() or {})


def test_login_unknown_username_rejected():
    client = _make_client()
    resp = client.post(
        "/api/auth/local/login",
        json={"username": _unique_username(), "password": "whatever123"},
    )
    assert resp.status_code == 401


def test_register_duplicate_username_conflict():
    client = _make_client()
    token = _default_token(client)
    username = _unique_username()

    first = client.post(
        "/api/auth/local/register",
        json={"username": username, "password": "long-enough-pw"},
        headers=_auth(token),
    )
    assert first.status_code == 201

    second = client.post(
        "/api/auth/local/register",
        json={"username": username, "password": "long-enough-pw"},
        headers=_auth(token),
    )
    assert second.status_code == 409


def test_register_rejects_short_password():
    client = _make_client()
    token = _default_token(client)
    resp = client.post(
        "/api/auth/local/register",
        json={"username": _unique_username(), "password": "short"},
        headers=_auth(token),
    )
    assert resp.status_code == 400


def test_registered_user_gets_default_groups():
    client = _make_client()
    token = _default_token(client)
    username = _unique_username()

    resp = client.post(
        "/api/auth/local/register",
        json={"username": username, "password": "long-enough-pw"},
        headers=_auth(token),
    )
    assert resp.status_code == 201

    login = client.post(
        "/api/auth/local/login",
        json={"username": username, "password": "long-enough-pw"},
    )
    user_token = login.get_json()["token"]

    groups = client.get("/api/groups", headers=_auth(user_token))
    assert groups.status_code == 200
    assert len(groups.get_json()) > 0
