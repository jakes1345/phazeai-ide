#!/usr/bin/env python3
"""Count lines (or characters, or words) in files, or in standard input."""

import argparse
import sys


def count_lines(text: str) -> int:
    return text.count("\n")


def count_chars(text: str) -> int:
    return len(text)


def count_words(text: str) -> int:
    return len(text.split())


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    what = parser.add_mutually_exclusive_group()
    what.add_argument("--chars", action="store_true", help="count characters")
    what.add_argument("--words", action="store_true", help="count words")
    parser.add_argument("files", nargs="*", help="files to read (default: stdin)")
    args = parser.parse_args(argv)

    count = count_chars if args.chars else count_words if args.words else count_lines
    if not args.files:
        print(count(sys.stdin.read()))
        return 0
    total = 0
    for path in args.files:
        with open(path, encoding="utf-8") as f:
            n = count(f.read())
        total += n
        print(f"{n} {path}")
    if len(args.files) > 1:
        print(f"{total} total")
    return 0


if __name__ == "__main__":
    sys.exit(main())
