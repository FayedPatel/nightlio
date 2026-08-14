# Ensure the api package is importable during tests
import os
import sys
from pathlib import Path

import pytest

root = Path(__file__).resolve().parent.parent
api_dir = root / "api"
if str(api_dir) not in sys.path:
    sys.path.insert(0, str(api_dir))

# Neutralize deployment-specific variables before any test imports the app.
# api/app.py calls load_dotenv() on the repo-root .env, which on a developer
# machine may hold a real OIDC/proxy configuration; load_dotenv never
# overrides variables that already exist in the environment, so presetting
# them to empty here pins the suite to the default self-host configuration
# regardless of what the local .env contains.
for _var in (
    "OIDC_ISSUER_URL",
    "OIDC_CLIENT_ID",
    "OIDC_CLIENT_SECRET",
    "OIDC_CALLBACK_URL",
    "FRONTEND_URL",
    "TRUST_PROXY_HEADERS",
):
    os.environ[_var] = ""


# Shared fixtures. Older test files still define their own `client` (a fixed
# /tmp/nightlio_test.db path plus manual resets); local fixtures shadow these,
# so they can migrate file-by-file. New tests should use these: tmp_path gives
# every test its own database, which keeps tests isolated and parallelizable.


@pytest.fixture()
def client(tmp_path, monkeypatch):
    from api.config import TestingConfig

    monkeypatch.setattr(
        TestingConfig, "DATABASE_PATH", str(tmp_path / "nightlio_test.db")
    )
    from api.app import create_app

    app = create_app("testing")
    with app.test_client() as test_client:
        yield test_client


@pytest.fixture()
def auth_headers(client):
    resp = client.post("/api/auth/local/login")
    assert resp.status_code == 200, resp.get_data(as_text=True)
    return {"Authorization": f"Bearer {resp.get_json()['token']}"}
