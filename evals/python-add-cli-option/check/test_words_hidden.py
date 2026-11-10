import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))


def wc(*args, stdin=""):
    out = subprocess.run(
        [sys.executable, os.path.join(HERE, "wc.py"), *args],
        input=stdin, capture_output=True, text=True, check=True,
    )
    return out.stdout


class Words(unittest.TestCase):
    def test_stdin(self):
        self.assertEqual(wc("--words", stdin="one two  three\nfour\t five\n"), "5\n")
        self.assertEqual(wc("--words", stdin=""), "0\n")

    def test_files_and_total(self):
        with tempfile.TemporaryDirectory() as d:
            a = os.path.join(d, "a.txt")
            b = os.path.join(d, "b.txt")
            with open(a, "w") as f:
                f.write("hello world\n")
            with open(b, "w") as f:
                f.write("  a b c  \n\n d\n")
            self.assertEqual(wc("--words", a, b), f"2 {a}\n4 {b}\n6 total\n")

    def test_other_counts_unchanged(self):
        self.assertEqual(wc(stdin="a b\nc\n"), "2\n")
        self.assertEqual(wc("--chars", stdin="a b\n"), "4\n")

    def test_options_exclude_each_other(self):
        r = subprocess.run(
            [sys.executable, os.path.join(HERE, "wc.py"), "--words", "--chars"],
            input="", capture_output=True, text=True,
        )
        self.assertNotEqual(r.returncode, 0)

    def test_readme_mentions_it(self):
        with open(os.path.join(HERE, "README.md")) as f:
            self.assertIn("--words", f.read())


if __name__ == "__main__":
    unittest.main()
