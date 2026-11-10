"""Parse human durations like "1h30m" into seconds."""


def parse_duration(text: str) -> int:
    """Return the number of seconds in ``text``.

    ``text`` is one or more parts, each a whole number followed by a unit:
    ``d`` (days), ``h`` (hours), ``m`` (minutes) or ``s`` (seconds). Units
    come in that order, each at most once, and may be separated by spaces:
    "1h30m", "2d", "90s", "1h 5m 10s". Units are case-insensitive.

    Raise ValueError for anything else, including an empty string, a
    negative or fractional number, a unit out of order or used twice.
    """
    raise NotImplementedError
