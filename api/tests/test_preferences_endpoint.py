"""Theme preference endpoints and the users.theme_preference migration."""

import sqlite3


def test_get_preferences_defaults_to_null_theme(client, auth_headers):
    resp = client.get("/api/preferences", headers=auth_headers)
    assert resp.status_code == 200
    assert resp.get_json() == {"theme": None}


def test_put_and_get_roundtrip(client, auth_headers):
    resp = client.put(
        "/api/preferences", json={"theme": "synthwave"}, headers=auth_headers
    )
    assert resp.status_code == 200
    assert resp.get_json()["theme"] == "synthwave"

    resp = client.get("/api/preferences", headers=auth_headers)
    assert resp.get_json() == {"theme": "synthwave"}


def test_put_accepts_every_allowed_theme(client, auth_headers):
    for theme in ("default", "light", "dark", "synthwave"):
        resp = client.put(
            "/api/preferences", json={"theme": theme}, headers=auth_headers
        )
        assert resp.status_code == 200, theme


def test_put_rejects_unknown_theme(client, auth_headers):
    resp = client.put(
        "/api/preferences", json={"theme": "hotdog-stand"}, headers=auth_headers
    )
    assert resp.status_code == 400


def test_preferences_require_auth(client):
    assert client.get("/api/preferences").status_code == 401
    assert client.put("/api/preferences", json={"theme": "dark"}).status_code == 401


def test_theme_column_added_to_legacy_database(tmp_path):
    """A pre-theme database gains theme_preference on init (upgrade path)."""
    legacy_path = tmp_path / "legacy.db"
    conn = sqlite3.connect(legacy_path)
    conn.execute(
        """
        CREATE TABLE users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            google_id TEXT UNIQUE NOT NULL,
            email TEXT NOT NULL,
            name TEXT NOT NULL,
            avatar_url TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            last_login TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )
        """
    )
    conn.execute(
        "INSERT INTO users (google_id, email, name) VALUES ('legacy', 'a@b.c', 'Legacy')"
    )
    conn.commit()
    conn.close()

    from api.database import MoodDatabase

    MoodDatabase(str(legacy_path))

    conn = sqlite3.connect(legacy_path)
    cols = {row[1] for row in conn.execute("PRAGMA table_info(users)")}
    row = conn.execute(
        "SELECT theme_preference FROM users WHERE google_id = 'legacy'"
    ).fetchone()
    conn.close()

    assert "theme_preference" in cols
    assert row[0] is None
