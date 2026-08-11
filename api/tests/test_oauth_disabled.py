from api.app import create_app


def test_oidc_routes_not_registered_when_disabled():
    app = create_app("testing")
    client = app.test_client()
    # With default config (no OIDC_ISSUER_URL), the oauth blueprint is not
    # registered -> expect 404.
    assert client.get("/api/auth/login/oidc").status_code in (404, 405)
    assert client.get("/api/auth/callback/oidc").status_code in (404, 405)


def test_legacy_google_routes_gone():
    app = create_app("testing")
    client = app.test_client()
    assert client.get("/api/auth/login/google").status_code in (404, 405)
    assert client.post("/api/auth/google", json={"token": "x"}).status_code in (
        404,
        405,
    )
