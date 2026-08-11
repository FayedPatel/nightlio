import uuid

from api.app import create_app


def test_handle_oidc_login_idempotent():
    app = create_app("testing")
    user_service = app.extensions["user_service"]

    # Simulate a validated OIDC identity payload
    sub = f"test-oidc-sub-{uuid.uuid4().hex[:12]}"
    email = f"{sub}@example.com"
    name = "Test User"
    avatar = "https://example.com/a.png"

    u1 = user_service.handle_oidc_login(sub, email, name, avatar)
    u2 = user_service.handle_oidc_login(sub, email, name, avatar)

    assert u1["id"] == u2["id"]
    assert u2["email"] == email
    assert u2["name"] == name


def test_handle_oidc_login_seeds_defaults_and_logs_login():
    app = create_app("testing")
    user_service = app.extensions["user_service"]
    db = user_service.db

    sub = f"test-oidc-sub-{uuid.uuid4().hex[:12]}"
    user = user_service.handle_oidc_login(sub, f"{sub}@example.com", "New User")

    # First login seeds the default groups for the new user
    groups = db.get_all_groups(user_id=user["id"])
    assert len(groups) > 0

    # And records a login event
    feed = db.get_activity(user["id"])
    assert any(a["event_type"] == "login" for a in feed)
