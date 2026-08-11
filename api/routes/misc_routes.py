import time
from flask import Blueprint, jsonify, request, send_file
import io
try:
    from markdown_pdf import MarkdownPdf, Section
except ImportError:
    MarkdownPdf = None
    Section = None

# /export/pdf takes no auth and has no rate limit, so this cap is the only
# thing standing between any caller with network access and a denial of
# service: MarkdownPdf renders synchronously inside whichever gunicorn
# worker handles the request, with no size/time limit of its own, so an
# arbitrarily large markdown payload would otherwise burn that worker's
# memory and CPU for as long as rendering takes. A single journal entry is
# a few KB; 1 MiB is a generous ceiling that still comfortably covers a
# very long entry while keeping a hostile payload from being economically
# viable to send.
MAX_PDF_CONTENT_SIZE = 1 * 1024 * 1024  # 1 MiB


def create_misc_routes():
    misc_bp = Blueprint("misc", __name__)

    @misc_bp.route("/")
    def health_check():
        return {
            "status": "healthy",
            "message": "Nightlio API is running",
            "timestamp": time.time(),
        }

    @misc_bp.route("/time")
    def get_current_time():
        return {"time": time.time()}

    @misc_bp.route("/export/pdf", methods=["POST"])
    def export_pdf():
        if not MarkdownPdf:
            return jsonify({"error": "markdown-pdf module not installed"}), 501
            
        data = request.get_json()
        if not data or "content" not in data:
            return jsonify({"error": "Content is required"}), 400
            
        content = data.get("content", "")

        # Measured in encoded bytes, not characters: a payload heavy in
        # multi-byte characters (emoji, non-Latin scripts) could otherwise
        # smuggle a much larger amount of actual data past a character
        # count check.
        content_size = len(content.encode("utf-8"))
        if content_size > MAX_PDF_CONTENT_SIZE:
            return (
                jsonify(
                    {
                        "error": (
                            "Content is too large to export "
                            f"(max {MAX_PDF_CONTENT_SIZE} bytes)"
                        )
                    }
                ),
                413,
            )

        pdf = MarkdownPdf()
        pdf.add_section(Section(content))
        
        out = io.BytesIO()
        pdf.save(out)
        out.seek(0)
        
        return send_file(
            out,
            mimetype="application/pdf",
            as_attachment=True,
            download_name="entry_export.pdf"
        )

    return misc_bp
