"""Tests for the httpOnly session cookie added alongside Bearer JWT auth.

Covers: the cookie is set on login with the right flags, a cookie-only
request can authenticate (no Authorization header), cookie-authenticated
mutations require the CSRF header, Bearer requests are exempt from that
CSRF check, and logout clears the cookie.

Uses a single module-scoped Flask app (one MoodDatabase/schema-init) shared
by every test in this file, with a fresh app.test_client() -- and thus a
fresh, isolated cookie jar -- per test. Creating a new app (and therefore a
new sqlite3.connect against the shared /tmp/nightlio_test.db) per test, as
some older test files do, is unnecessary here and was observed to make an
already file-locking-sensitive CI sandbox (WSL2) noticeably flakier under
this file's higher app-creation count; the module-scoped fixture avoids
adding to that without losing per-test isolation (each test gets its own
client/cookie jar, so no session data leaks between tests).
"""

import uuid

import pytest

from api.app import create_app
from api.utils.auth_cookies import COOKIE_NAME


@pytest.fixture(scope="module")
def app():
    return create_app("testing")


def _client(app):
    return app.test_client()


def _default_login(client):
    """Log in via the credential-free self-host path.

    Using the test client's built-in cookie jar, this also leaves the
    session cookie attached to `client` for subsequent requests.
    """
    resp = client.post("/api/auth/local/login")
    assert resp.status_code == 200
    return resp


def _set_cookie_headers(resp):
    """All Set-Cookie header values on a response, as a list of strings."""
    return resp.headers.get_all("Set-Cookie")


def _unique_group_name():
    # Group names are unique per user; the default self-host user is shared
    # across every test in this module (and across repeated test runs
    # against the same on-disk test DB), so a fixed literal name would
    # eventually collide.
    return f"cookie-test-group-{uuid.uuid4().hex[:10]}"


def test_login_sets_httponly_samesite_cookie(app):
    client = _client(app)
    resp = _default_login(client)

    set_cookie_headers = _set_cookie_headers(resp)
    matching = [h for h in set_cookie_headers if h.startswith(f"{COOKIE_NAME}=")]
    assert len(matching) == 1
    cookie_header = matching[0]

    assert "HttpOnly" in cookie_header
    assert "SameSite=Lax" in cookie_header
    assert "Path=/" in cookie_header
    # Plain HTTP test client requests are not secure; Secure must be absent
    # so the cookie still works over http self-host deployments.
    assert "Secure" not in cookie_header


def test_credentialed_login_also_sets_cookie(app):
    setup_client = _client(app)
    token = _default_login(setup_client).get_json()["token"]

    username = f"cookie-test-user-{uuid.uuid4().hex[:10]}"
    password = "correct horse battery staple"
    reg = setup_client.post(
        "/api/auth/local/register",
        json={"username": username, "password": password},
        headers={"Authorization": f"Bearer {token}"},
    )
    assert reg.status_code == 201

    # Fresh client (no cookie jar carryover) doing a credentialed login.
    fresh_client = _client(app)
    resp = fresh_client.post(
        "/api/auth/local/login",
        json={"username": username, "password": password},
    )
    assert resp.status_code == 200
    matching = [
        h for h in _set_cookie_headers(resp) if h.startswith(f"{COOKIE_NAME}=")
    ]
    assert len(matching) == 1
    assert "HttpOnly" in matching[0]


def test_cookie_authenticates_get_request_without_authorization_header(app):
    client = _client(app)
    _default_login(client)  # cookie now stored in client's jar

    # No Authorization header at all -- cookie alone must authenticate.
    resp = client.get("/api/groups")
    assert resp.status_code == 200


def test_cookie_auth_post_without_csrf_header_rejected(app):
    client = _client(app)
    _default_login(client)

    resp = client.post(
        "/api/groups",
        json={"name": _unique_group_name()},
        # Deliberately no X-Requested-With header.
    )
    assert resp.status_code == 403


def test_cookie_auth_post_with_csrf_header_succeeds(app):
    client = _client(app)
    _default_login(client)

    resp = client.post(
        "/api/groups",
        json={"name": _unique_group_name()},
        headers={"X-Requested-With": "nightlio"},
    )
    assert resp.status_code == 201


def test_cookie_auth_post_wrong_content_type_rejected_even_with_csrf_header(app):
    client = _client(app)
    _default_login(client)

    resp = client.post(
        "/api/groups",
        data="name=not-json",
        content_type="application/x-www-form-urlencoded",
        headers={"X-Requested-With": "nightlio"},
    )
    assert resp.status_code == 403


def test_bearer_auth_post_skips_csrf_checks(app):
    setup_client = _client(app)
    token = _default_login(setup_client).get_json()["token"]

    # Fresh client with no cookie at all -- pure Bearer auth, no
    # X-Requested-With header, and the cookie-auth CSRF check must not apply.
    fresh_client = _client(app)
    resp = fresh_client.post(
        "/api/groups",
        json={"name": _unique_group_name()},
        headers={"Authorization": f"Bearer {token}"},
    )
    assert resp.status_code == 201


def test_bearer_header_wins_over_cookie_when_both_present(app):
    client = _client(app)
    token = _default_login(client).get_json()["token"]  # cookie now in jar too

    # Bearer present alongside the cookie: request must still be treated as
    # header-auth (no CSRF check applied), proving the header takes priority.
    resp = client.post(
        "/api/groups",
        json={"name": _unique_group_name()},
        headers={"Authorization": f"Bearer {token}"},
    )
    assert resp.status_code == 201


def test_no_credentials_at_all_rejected(app):
    client = _client(app)
    resp = client.get("/api/groups")
    assert resp.status_code == 401


def test_logout_clears_cookie_and_ends_cookie_session(app):
    client = _client(app)
    _default_login(client)

    # Cookie session works before logout.
    assert client.get("/api/groups").status_code == 200

    logout_resp = client.post(
        "/api/auth/logout", headers={"X-Requested-With": "nightlio"}
    )
    assert logout_resp.status_code == 200
    matching = [
        h for h in _set_cookie_headers(logout_resp) if h.startswith(f"{COOKIE_NAME}=")
    ]
    assert len(matching) == 1
    # Cleared cookies are expressed as an empty value with Max-Age=0 (or an
    # expiry in the past); accept either since both invalidate the cookie.
    assert "Max-Age=0" in matching[0] or "1970" in matching[0]

    # After logout, the cookie no longer authenticates.
    assert client.get("/api/groups").status_code == 401


def test_logout_without_any_session_is_a_no_op_success(app):
    client = _client(app)
    resp = client.post("/api/auth/logout")
    assert resp.status_code == 200


def test_verify_endpoint_works_with_cookie_only(app):
    client = _client(app)
    login_resp = _default_login(client)
    user_id = login_resp.get_json()["user"]["id"]

    resp = client.post(
        "/api/auth/verify", json={}, headers={"X-Requested-With": "nightlio"}
    )
    assert resp.status_code == 200
    assert resp.get_json()["user"]["id"] == user_id


def test_verify_endpoint_still_works_with_bearer_only(app):
    setup_client = _client(app)
    token = _default_login(setup_client).get_json()["token"]

    fresh_client = _client(app)
    resp = fresh_client.post(
        "/api/auth/verify", headers={"Authorization": f"Bearer {token}"}
    )
    assert resp.status_code == 200
