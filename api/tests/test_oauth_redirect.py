"""Tests for the OIDC callback redirect target validation.

The SSO token is handed to the SPA in the URL fragment of a redirect built
from FRONTEND_URL. A FRONTEND_URL pointing at a foreign origin would leak
that fragment (and the token in it) off-host, so _frontend_redirect must
only honor a relative path or a same-host http(s) URL and fall back to a
same-origin redirect for anything else.
"""

from types import SimpleNamespace

import pytest
from flask import Flask

import api.config
from api.auth.oauth import _frontend_redirect


@pytest.fixture()
def app():
    return Flask(__name__)


def _redirect_with_frontend_url(app, monkeypatch, frontend_url):
    monkeypatch.setattr(
        api.config, "get_config", lambda: SimpleNamespace(FRONTEND_URL=frontend_url)
    )
    # Default test request context host is "localhost".
    with app.test_request_context("/api/auth/callback/oidc"):
        response = _frontend_redirect("sso_token=dummy")
    return response.headers["Location"]


def test_empty_frontend_url_redirects_same_origin(app, monkeypatch):
    location = _redirect_with_frontend_url(app, monkeypatch, None)
    assert location == "/login#sso_token=dummy"


def test_relative_frontend_url_is_honored(app, monkeypatch):
    location = _redirect_with_frontend_url(app, monkeypatch, "/app/")
    assert location == "/app/login#sso_token=dummy"


def test_same_origin_absolute_frontend_url_is_honored(app, monkeypatch):
    location = _redirect_with_frontend_url(app, monkeypatch, "http://localhost")
    assert location == "http://localhost/login#sso_token=dummy"


def test_foreign_origin_frontend_url_falls_back_to_same_origin(app, monkeypatch):
    location = _redirect_with_frontend_url(
        app, monkeypatch, "https://evil.example.com"
    )
    assert "evil.example.com" not in location
    assert location == "/login#sso_token=dummy"


def test_scheme_relative_frontend_url_is_rejected(app, monkeypatch):
    location = _redirect_with_frontend_url(app, monkeypatch, "//evil.example.com")
    assert "evil.example.com" not in location
    assert location == "/login#sso_token=dummy"


def test_backslash_frontend_url_is_rejected(app, monkeypatch):
    # Some browsers treat "/\" like "//", turning a "path" into a host.
    location = _redirect_with_frontend_url(app, monkeypatch, "/\\evil.example.com")
    assert "evil.example.com" not in location
    assert location == "/login#sso_token=dummy"
