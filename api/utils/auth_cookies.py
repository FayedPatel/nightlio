"""Shared helpers for the httpOnly session cookie.

Background
----------
The JWT was previously only ever handed to the SPA and stored in
``localStorage`` (readable by any script running on the page — an
acceptable tradeoff for a single-user self-host instance, worse as
multi-user OIDC becomes central). This module adds a second, parallel
delivery path: the same JWT is also set as an httpOnly cookie so the
browser holds a copy JavaScript cannot read. ``Authorization: Bearer``
keeps working unchanged — see ``api/utils/auth_middleware.py``, which
prefers the header and only falls back to this cookie.

Cookie attributes
------------------
- ``HttpOnly`` — never exposed to ``document.cookie`` / page scripts.
- ``SameSite=Lax`` — sent on top-level navigations and same-site requests,
  withheld on cross-site subresource/XHR requests. Combined with the CSRF
  checks in ``auth_middleware.require_auth`` (custom header requirement,
  which forces a CORS preflight; JSON-only content type, which a plain HTML
  form cannot send) this is the project's CSRF defense for cookie-authed
  requests. Header-authenticated (Bearer) requests are not CSRF-able —
  cross-site pages have no way to read another origin's localStorage token
  to attach it — so they skip these checks entirely.
- ``Secure`` — set only when the request is judged to be HTTPS (see
  ``_is_https_request`` below). Self-hoster note: on a plain HTTP
  deployment the cookie is issued without ``Secure`` (SameSite=Lax alone
  still applies) so login keeps working over HTTP; browsers will not
  transmit a ``Secure`` cookie over HTTP at all, so getting this flag wrong
  in either direction breaks login, not just weakens it.
- ``Path=/`` — sent on every API route under this origin.
- ``Max-Age`` — mirrors the JWT's own expiry (``JWT_ACCESS_TOKEN_EXPIRES``)
  so the cookie does not outlive the token it carries.

Reverse proxy note
-------------------
If Nightlio sits behind a reverse proxy that terminates TLS (nginx,
Traefik, Cloudflare, etc.), Flask sees a plain HTTP connection from the
proxy and would otherwise mark the cookie non-Secure even though the
browser-to-proxy hop is HTTPS. Set ``TRUST_PROXY_HEADERS=1`` (the same
flag ``api/utils/rate_limiter.py`` uses for ``X-Forwarded-For``) so this
module also honors ``X-Forwarded-Proto`` from that proxy. Leave it unset
if the app is reached directly, or the proxy does not set/strip that
header, or you allow forging an HTTPS Secure cookie over what is actually
a plaintext connection.
"""

import os

from flask import request

try:
    from api.utils.is_truthy import is_truthy
except ImportError:  # pragma: no cover - fallback for running from inside api/
    from utils.is_truthy import is_truthy  # type: ignore

COOKIE_NAME = "nightlio_token"

# Requests with this header on a cookie-authenticated mutation prove the
# caller is same-site JavaScript (the header forces a CORS preflight, which
# a plain cross-site HTML form submission cannot trigger). Bearer-token
# requests never need it; see auth_middleware.require_auth.
CSRF_HEADER_NAME = "X-Requested-With"
CSRF_HEADER_VALUE = "nightlio"


def _is_https_request() -> bool:
    """Best-effort HTTPS detection, proxy-aware only when opted in."""
    if request.is_secure:
        return True
    if is_truthy(os.environ.get("TRUST_PROXY_HEADERS")):
        proto = request.headers.get("X-Forwarded-Proto", "")
        return proto.split(",")[0].strip().lower() == "https"
    return False


def set_auth_cookie(response, token: str, max_age_seconds: int):
    """Attach the httpOnly session cookie to a Flask response, in place."""
    response.set_cookie(
        COOKIE_NAME,
        token,
        max_age=max_age_seconds,
        path="/",
        httponly=True,
        samesite="Lax",
        secure=_is_https_request(),
    )
    return response


def clear_auth_cookie(response):
    """Remove the session cookie (used by logout)."""
    response.set_cookie(
        COOKIE_NAME,
        "",
        max_age=0,
        expires=0,
        path="/",
        httponly=True,
        samesite="Lax",
        secure=_is_https_request(),
    )
    return response
