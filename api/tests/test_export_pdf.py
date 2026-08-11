"""Tests for the unauthenticated /api/export/pdf endpoint's size cap.

/api/export/pdf takes no auth token and no rate limit, so
api/routes/misc_routes.py:MAX_PDF_CONTENT_SIZE is the only thing standing
between any caller with network access and a denial of service: without
it, an attacker could send an arbitrarily large markdown payload (up to
Flask's own request-body ceiling) and force MarkdownPdf to render it
synchronously inside a gunicorn worker, burning that worker's memory and
CPU for as long as rendering takes.

Note on test design: these tests monkeypatch MAX_PDF_CONTENT_SIZE down to
a small value rather than actually sending content near the real 1 MiB
default. A large *unbroken* payload (no whitespace to break lines on) was
observed during development to make MarkdownPdf's layout pass take
minutes for a single request on ordinary hardware -- a small-scale
demonstration of exactly the resource-exhaustion risk this cap exists to
close, but not something a test suite should reproduce on every run.
"""

import api.routes.misc_routes as misc_routes_module
from api.app import create_app


def _client():
    app = create_app("testing")
    return app.test_client()


def test_default_max_pdf_content_size_is_one_mebibyte():
    # Pins the documented production default; a change here should be a
    # deliberate decision, not a silent drift.
    assert misc_routes_module.MAX_PDF_CONTENT_SIZE == 1 * 1024 * 1024


def test_export_pdf_rejects_oversized_content(monkeypatch):
    monkeypatch.setattr(misc_routes_module, "MAX_PDF_CONTENT_SIZE", 100)
    oversized = "a " * 100  # 200 bytes of whitespace-separated content
    resp = _client().post("/api/export/pdf", json={"content": oversized})
    assert resp.status_code == 413
    assert "too large" in resp.get_json()["error"].lower()


def test_export_pdf_accepts_content_at_the_limit(monkeypatch):
    # Exactly at the cap must still be accepted -- the check is "over the
    # limit", not "at or over".
    monkeypatch.setattr(misc_routes_module, "MAX_PDF_CONTENT_SIZE", 200)
    at_limit = "hello world " * 16  # 192 bytes, whitespace-broken
    assert len(at_limit.encode("utf-8")) <= 200
    resp = _client().post("/api/export/pdf", json={"content": at_limit})
    assert resp.status_code == 200
    assert resp.mimetype == "application/pdf"


def test_export_pdf_still_works_for_normal_content():
    resp = _client().post(
        "/api/export/pdf", json={"content": "# Hello\n\nA normal journal entry."}
    )
    assert resp.status_code == 200
    assert resp.mimetype == "application/pdf"


def test_export_pdf_requires_content_field():
    resp = _client().post("/api/export/pdf", json={})
    assert resp.status_code == 400


def test_export_pdf_size_check_counts_encoded_bytes_not_characters(monkeypatch):
    # Multi-byte characters (e.g. emoji) must be counted by their UTF-8
    # byte length, not by Python string length, or a payload using them
    # could smuggle several times MAX_PDF_CONTENT_SIZE actual bytes past
    # a naive len(content) check.
    monkeypatch.setattr(misc_routes_module, "MAX_PDF_CONTENT_SIZE", 100)
    emoji_char = "\U0001f600"  # 4 bytes in UTF-8, 1 Python character
    content = emoji_char * 30  # 30 chars (< 100) but 120 bytes (> 100)
    assert len(content) < 100
    assert len(content.encode("utf-8")) > 100

    resp = _client().post("/api/export/pdf", json={"content": content})
    assert resp.status_code == 413
