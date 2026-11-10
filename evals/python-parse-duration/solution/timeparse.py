"""Parse human durations like "1h30m" into seconds."""

import re

_PART = re.compile(r"(\d+)([dhms])", re.IGNORECASE)
_SECONDS = {"d": 86400, "h": 3600, "m": 60, "s": 1}
_ORDER = "dhms"


def parse_duration(text: str) -> int:
    """Return the number of seconds in ``text`` (see the task)."""
    parts = text.split(" ") if text else []
    total = 0
    last = -1
    found = False
    for chunk in parts:
        if not chunk:
            raise ValueError(f"bad duration: {text!r}")
        pos = 0
        for m in _PART.finditer(chunk):
            if m.start() != pos:
                raise ValueError(f"bad duration: {text!r}")
            unit = m.group(2).lower()
            index = _ORDER.index(unit)
            if index <= last:
                raise ValueError(f"bad duration: {text!r}")
            last = index
            total += int(m.group(1)) * _SECONDS[unit]
            pos = m.end()
            found = True
        if pos != len(chunk):
            raise ValueError(f"bad duration: {text!r}")
    if not found:
        raise ValueError(f"bad duration: {text!r}")
    return total
