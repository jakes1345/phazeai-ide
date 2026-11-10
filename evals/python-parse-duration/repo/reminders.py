"""Tiny reminder scheduler built on parse_duration."""

import time

from timeparse import parse_duration


def due_at(after: str, now: float | None = None) -> float:
    """The Unix time a reminder set ``after`` from ``now`` goes off."""
    start = time.time() if now is None else now
    return start + parse_duration(after)
