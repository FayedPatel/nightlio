import pytest

import api.config as config_module
from api.app import create_app


def _client():
    app = create_app("testing")
    return app.test_client()


def test_config_endpoint_client():
    client = _client()
    resp = client.get("/api/config")
    assert resp.status_code == 200
    data = resp.get_json()
    assert set(data.keys()) == {
        "enable_oidc",
        "enable_mood_music",
        "signup_url",
    }
    assert isinstance(data["enable_oidc"], bool)
    assert isinstance(data["enable_mood_music"], bool)
    assert data["signup_url"] is None or isinstance(data["signup_url"], str)


def test_config_endpoint_signup_url_null_when_unset(monkeypatch):
    monkeypatch.delenv("OIDC_SIGNUP_URL", raising=False)
    monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)
    data = _client().get("/api/config").get_json()
    assert data["signup_url"] is None


def test_config_endpoint_signup_url_returned_when_oidc_enabled(monkeypatch):
    monkeypatch.setenv("OIDC_ISSUER_URL", "https://id.example.com")
    monkeypatch.setenv("OIDC_SIGNUP_URL", "https://id.example.com/signup")
    monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)
    try:
        data = _client().get("/api/config").get_json()
        assert data["enable_oidc"] is True
        assert data["signup_url"] == "https://id.example.com/signup"
    finally:
        # Force a reload for subsequent tests once monkeypatch restores env.
        monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)


@pytest.mark.parametrize(
    "bad_url",
    [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:text/html,hi",
        "id.example.com/signup",  # scheme-less
        "https://",  # no host
    ],
)
def test_config_endpoint_signup_url_null_for_unsafe_values(monkeypatch, bad_url):
    # The value lands in an <a href> on the login page; anything that is not
    # an absolute http(s) URL must be dropped server-side.
    monkeypatch.setenv("OIDC_ISSUER_URL", "https://id.example.com")
    monkeypatch.setenv("OIDC_SIGNUP_URL", bad_url)
    monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)
    try:
        data = _client().get("/api/config").get_json()
        assert data["signup_url"] is None
    finally:
        monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)


def test_config_endpoint_signup_url_null_when_oidc_off_even_if_set(monkeypatch):
    monkeypatch.delenv("OIDC_ISSUER_URL", raising=False)
    monkeypatch.setenv("OIDC_SIGNUP_URL", "https://id.example.com/signup")
    monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)
    try:
        data = _client().get("/api/config").get_json()
        assert data["enable_oidc"] is False
        assert data["signup_url"] is None
    finally:
        monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)
