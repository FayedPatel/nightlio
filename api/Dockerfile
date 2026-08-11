FROM python:3.11-slim

# Set working directory
WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y \
    gcc \
    && rm -rf /var/lib/apt/lists/*

# Copy requirements first for better caching
COPY requirements.txt .
COPY requirements-optional.txt .

# Install Python dependencies
RUN pip install --no-cache-dir -r requirements.txt

# Copy the entire API directory
COPY . .

# Create symbolic link for import compatibility
RUN ln -sf /app /app/api

# Create data directory for SQLite database (holds nightlio.db and the
# rate-limit counter db opened directly by api/database.py and
# api/utils/rate_limiter.py); must stay writable by the non-root runtime
# user set up below.
RUN mkdir -p /app/data

# Run as an unprivileged user. There was previously no USER directive here,
# so the process ran as root (uid 0) by default: a compromised dependency
# or a vulnerability reachable from any HTTP route (this app renders
# attacker-supplied markdown to PDF at the unauthenticated /api/export/pdf
# route) would execute with full root privileges inside the container --
# able to modify any root-owned file, install persistence, or hand a
# container-escape bug root to start from. Switching to a dedicated,
# unprivileged account confines a compromised process to what it actually
# needs: read access to the app code and write access to /app/data (the
# only two things this container writes at runtime), nothing else.
#
# Self-hoster note: the nightlio_data volume is chowned to this uid (1000)
# the first time it's populated from the image. If you have an existing
# volume from an older image that ran as root, its files stay owned by
# root/0 across the upgrade and this new unprivileged user will fail to
# write to them; fix an existing volume once with:
#   docker run --rm -v nightlio_data:/data alpine chown -R 1000:1000 /data
RUN useradd --create-home --uid 1000 --shell /usr/sbin/nologin appuser \
    && chown -R appuser:appuser /app/data
USER appuser

# Expose port
EXPOSE 5000

# Set environment variables
ENV FLASK_APP=start.py
ENV PYTHONPATH=/app:$PYTHONPATH

# Health check
HEALTHCHECK --interval=30s --timeout=30s --start-period=5s --retries=3 \
    CMD python -c "import requests; requests.get('http://localhost:5000/api/')" || exit 1

# Run the application
CMD ["python", "docker_start.py"]

# Alternative production command with Gunicorn (uncomment for production)
# CMD ["gunicorn", "--bind", "[::]:5000", "--workers", "4", "docker_start:app"]
