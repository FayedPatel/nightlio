import re
from datetime import date as _date
from datetime import datetime, timedelta
from typing import Any, Dict, List

# Both date shapes that exist in stored data: ISO (what the frontend sends
# now) and legacy US locale (M/D/YYYY from toLocaleDateString()).
ENTRY_DATE_FORMATS = ("%Y-%m-%d", "%m/%d/%Y")


def validate_entry_date(date_str: Any) -> str:
    """Validate a mood-entry date string and return it unchanged.

    Raises ValueError for unparseable values or future dates; one day of
    slack tolerates client/server timezone skew.
    """
    value = str(date_str)
    for fmt in ENTRY_DATE_FORMATS:
        try:
            parsed = datetime.strptime(value, fmt).date()
            break
        except ValueError:
            continue
    else:
        raise ValueError("date must be YYYY-MM-DD or M/D/YYYY")

    if parsed > _date.today() + timedelta(days=1):
        raise ValueError("date cannot be in the future")

    return value


def validate_completion_date(date_str: Any) -> str:
    """Validate a goal-completion date and return it normalized to ISO.

    Accepts the same shapes as entry dates but always returns YYYY-MM-DD,
    since goal_completions.date is ISO-only. Same one-day future slack.
    """
    value = str(date_str)
    for fmt in ENTRY_DATE_FORMATS:
        try:
            parsed = datetime.strptime(value, fmt).date()
            break
        except ValueError:
            continue
    else:
        raise ValueError("date must be YYYY-MM-DD or M/D/YYYY")

    if parsed > _date.today() + timedelta(days=1):
        raise ValueError("date cannot be in the future")

    return parsed.strftime("%Y-%m-%d")


def validate_mood_entry(data: Dict[str, Any]) -> List[str]:
    """Validate mood entry data"""
    errors = []

    # Validate mood value
    mood = data.get("mood")
    if not isinstance(mood, int) or not (1 <= mood <= 5):
        errors.append("Mood must be an integer between 1 and 5")

    # Validate content
    content = data.get("content", "").strip()
    if not content:
        errors.append("Content cannot be empty")
    elif len(content) > 10000:  # 10KB limit
        errors.append("Content too long (max 10,000 characters)")

    # Validate date format
    date = data.get("date", "")
    if not re.match(r"^\d{1,2}/\d{1,2}/\d{4}$", date):
        errors.append("Invalid date format (expected MM/DD/YYYY)")

    # Validate selected options
    selected_options = data.get("selected_options", [])
    if not isinstance(selected_options, list):
        errors.append("Selected options must be a list")
    elif len(selected_options) > 50:  # Reasonable limit
        errors.append("Too many selected options (max 50)")

    return errors


def validate_group_data(data: Dict[str, Any]) -> List[str]:
    """Validate group creation data"""
    errors = []

    name = data.get("name", "").strip()
    if not name:
        errors.append("Group name cannot be empty")
    elif len(name) > 100:
        errors.append("Group name too long (max 100 characters)")
    elif not re.match(r"^[a-zA-Z0-9\s\-_]+$", name):
        errors.append("Group name contains invalid characters")

    return errors


def sanitize_string(value: str, max_length: int = 1000) -> str:
    """Sanitize string input"""
    if not isinstance(value, str):
        return ""

    # Remove null bytes and control characters
    sanitized = re.sub(r"[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]", "", value)

    # Trim whitespace and limit length
    return sanitized.strip()[:max_length]
