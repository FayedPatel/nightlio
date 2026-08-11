#!/bin/sh
# Run the backend test suite, auto-detecting which Python virtualenv to use.
#
# Priority: an already-active virtualenv ($VIRTUAL_ENV), then api/venv,
# then .venv, then fall back to whatever python3/python is on PATH.

set -e

PYTHONPATH=.
export PYTHONPATH

if [ -n "$VIRTUAL_ENV" ] && [ -x "$VIRTUAL_ENV/bin/pytest" ]; then
    exec "$VIRTUAL_ENV/bin/pytest" "$@"
elif [ -x "api/venv/bin/pytest" ]; then
    exec api/venv/bin/pytest "$@"
elif [ -x ".venv/bin/pytest" ]; then
    exec .venv/bin/pytest "$@"
elif command -v python3 >/dev/null 2>&1; then
    exec python3 -m pytest "$@"
else
    exec python -m pytest "$@"
fi
