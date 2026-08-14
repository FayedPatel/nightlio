"""DISABLE_LOCAL_LOGIN hard-refuses POST /api/auth/local/login entirely —
both the username/password form and credential-free self-host mode — so an
SSO-only deployment has exactly one door."""

import pytest

import api.config as config_module
from api.app import create_app


@pytest.fixture()
def disabled_client(tmp_path, monkeypatch):
    from api.config import TestingConfig

    monkeypatch.setattr(
        TestingConfig, "DATABASE_PATH", str(tmp_path / "nightlio_test.db")
    )
    monkeypatch.setenv("DISABLE_LOCAL_LOGIN", "1")
    monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)
    app = create_app("testing")
    try:
        with app.test_client() as client:
            yield client
    finally:
        # Force a reload for subsequent tests once monkeypatch restores env.
        monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)


def test_credential_free_login_refused(disabled_client):
    resp = disabled_client.post("/api/auth/local/login")
    assert resp.status_code == 403
    assert "disabled" in resp.get_json()["error"].lower()


def test_credentialed_login_refused(disabled_client):
    resp = disabled_client.post(
        "/api/auth/local/login",
        json={"username": "someone", "password": "hunter2"},
    )
    assert resp.status_code == 403


def test_config_reports_local_login_disabled(disabled_client):
    data = disabled_client.get("/api/config").get_json()
    assert data["enable_local_login"] is False


def test_default_keeps_local_login_enabled(client):
    data = client.get("/api/config").get_json()
    assert data["enable_local_login"] is True
    assert client.post("/api/auth/local/login").status_code == 200
