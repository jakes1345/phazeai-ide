import unittest

from reminders import due_at
from timeparse import parse_duration


class ParseDuration(unittest.TestCase):
    def test_single_units(self):
        self.assertEqual(parse_duration("90s"), 90)
        self.assertEqual(parse_duration("2d"), 172800)
        self.assertEqual(parse_duration("15m"), 900)
        self.assertEqual(parse_duration("0s"), 0)

    def test_combined(self):
        self.assertEqual(parse_duration("1h30m"), 5400)
        self.assertEqual(parse_duration("1h 5m 10s"), 3910)
        self.assertEqual(parse_duration("1d2h3m4s"), 93784)

    def test_case_insensitive(self):
        self.assertEqual(parse_duration("1H"), 3600)
        self.assertEqual(parse_duration("2M3S"), 123)

    def test_rejects(self):
        for bad in ["", "h", "10", "1x", "1m1h", "1h1h", "-1h", "1.5h", "h1", "1 h"]:
            with self.subTest(bad=bad):
                with self.assertRaises(ValueError):
                    parse_duration(bad)

    def test_used_by_reminders(self):
        self.assertEqual(due_at("1m", now=100.0), 160.0)


if __name__ == "__main__":
    unittest.main()
