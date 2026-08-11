# Ensure the api package is importable during tests
import os
import sys
from pathlib import Path

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
